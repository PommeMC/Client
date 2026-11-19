use std::time::Instant;

use glam::DVec3;

use crate::renderer::camera::{Camera, CameraUniform};
use crate::renderer::{rhi, shader, util};
use crate::resources::{ResourceManager, resource_id};
use crate::world::border::BorderRenderState;

/// `WorldBorderRenderer.RENDERING_OFFSET`: the walls sit this far inside.
const RENDERING_OFFSET: f64 = 0.01;

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

        // TODO: vanilla's `MipmappedTexture` gives the forcefield four mip
        // levels; this one has none.
        let sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST_REPEAT);

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
        border: &BorderRenderState,
        render_distance_blocks: f32,
    ) {
        let time_secs = self.start_time.elapsed().as_secs_f32();
        let cam = *camera.position;
        let radius = f64::from(render_distance_blocks);

        let bounds = border.bounds;
        let dist_to_border = bounds.distance_to(cam.x, cam.z);
        let min_x = bounds.min_x + RENDERING_OFFSET;
        let max_x = bounds.max_x - RENDERING_OFFSET;
        let min_z = bounds.min_z + RENDERING_OFFSET;
        let max_z = bounds.max_z - RENDERING_OFFSET;

        if cam.x < min_x - radius
            || cam.x > max_x + radius
            || cam.z < min_z - radius
            || cam.z > max_z + radius
            || dist_to_border >= radius
        {
            return;
        }

        let alpha = (1.0 - dist_to_border / radius).powi(4).clamp(0.0, 1.0) as f32;
        if alpha <= 0.0 {
            return;
        }
        let [red, green, blue] = border.tint;
        let color = [red, green, blue, alpha];

        let anchor = camera.anchor();
        let anim = time_secs.rem_euclid(3.0) / 3.0;

        // `WorldBorderRenderer.prepare`: the walls reach the far plane above
        // and below the camera, the texture running half a tile a block.
        let half_height = f64::from(camera.planes().1);
        let (y_bot, y_top) = (cam.y - half_height, cam.y + half_height);
        let v_top = -((cam.y * 0.5).rem_euclid(1.0) as f32) + anim;
        let v_bot = v_top + half_height as f32;

        let z_start = (cam.z - radius).floor().clamp(min_z, max_z);
        let z_end = (cam.z + radius).ceil().clamp(min_z, max_z);
        let u0_z = (z_start.floor() as i64 & 1) as f32 * 0.5 + anim;
        let u_z = [u0_z, u0_z + (z_end - z_start) as f32 * 0.5];

        let x_start = (cam.x - radius).floor().clamp(min_x, max_x);
        let x_end = (cam.x + radius).ceil().clamp(min_x, max_x);
        let u0_x = (x_start.floor() as i64 & 1) as f32 * 0.5 + anim;
        let u_x = [u0_x, u0_x + (x_end - x_start) as f32 * 0.5];

        let wall = |from: (f64, f64), to: (f64, f64)| {
            [
                DVec3::new(from.0, y_bot, from.1),
                DVec3::new(to.0, y_bot, to.1),
                DVec3::new(to.0, y_top, to.1),
                DVec3::new(from.0, y_top, from.1),
            ]
        };
        // West, east, north, south, each with the camera's distance inside it.
        let corners = [
            (wall((min_x, z_start), (min_x, z_end)), u_z),
            (wall((max_x, z_end), (max_x, z_start)), u_z),
            (wall((x_end, min_z), (x_start, min_z)), u_x),
            (wall((x_start, max_z), (x_end, max_z)), u_x),
        ];
        let mut walls: Vec<_> = bounds
            .distances_to(cam.x, cam.z)
            .into_iter()
            .zip(corners)
            .collect();
        // `WorldBorderRenderState.closestBorder`: the nearest first, as each
        // writes its depth over the ones behind it.
        walls.sort_by(|a, b| a.0.total_cmp(&b.0));

        let mut verts: Vec<WorldBorderVertex> = Vec::with_capacity(24);
        for (distance, (corners, [u0, u1])) in walls {
            if distance < radius {
                push_wall_quad(&mut verts, anchor, corners, [u0, u1, v_bot, v_top], color);
            }
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
    corners: [DVec3; 4],
    uv: [f32; 4],
    color: [f32; 4],
) {
    let v0_pos = (corners[0] - anchor).as_vec3().to_array();
    let v1_pos = (corners[1] - anchor).as_vec3().to_array();
    let v2_pos = (corners[2] - anchor).as_vec3().to_array();
    let v3_pos = (corners[3] - anchor).as_vec3().to_array();

    verts.push(WorldBorderVertex {
        position: v0_pos,
        uv: [uv[0], uv[2]],
        color,
    });
    verts.push(WorldBorderVertex {
        position: v1_pos,
        uv: [uv[1], uv[2]],
        color,
    });
    verts.push(WorldBorderVertex {
        position: v2_pos,
        uv: [uv[1], uv[3]],
        color,
    });

    verts.push(WorldBorderVertex {
        position: v0_pos,
        uv: [uv[0], uv[2]],
        color,
    });
    verts.push(WorldBorderVertex {
        position: v2_pos,
        uv: [uv[1], uv[3]],
        color,
    });
    verts.push(WorldBorderVertex {
        position: v3_pos,
        uv: [uv[0], uv[3]],
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
                write: true,
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
