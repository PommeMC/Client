use std::path::Path;

use glam::camera::rh::proj;
use glam::{Mat4, Vec3};

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::chunk::block_lighter::LightLevels;
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::{SkinData, rhi, shader, util};
const NEAR: f32 = 0.05;
const FAR: f32 = 10.0;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HandVertex {
    position: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HandUniform {
    mvp: [[f32; 4]; 4],
    /// x = the player's light lane.
    light: [f32; 4],
}

pub struct HandPipeline {
    pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    mvp_layout: rhi::backend::BindGroupLayout,
    skin_layout: rhi::backend::BindGroupLayout,
    mvp_sets: Vec<rhi::backend::BindGroup>,
    skin_set: rhi::backend::BindGroup,
    mvp_buffers: Vec<rhi::backend::Buffer>,
    /// The shared [`Lightmap`]'s group, set 2.
    lightmap_set: rhi::backend::BindGroup,
    vertex_buffer: rhi::backend::Buffer,
    /// Per arm: the buffer holds the right arm, then the left.
    vertex_count: u32,
    skin: rhi::backend::Texture,
    skin_sampler: rhi::backend::Sampler,
}

impl HandPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        lightmap: &Lightmap,
    ) -> Self {
        let mvp_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let skin_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[mvp_layout, skin_layout, lightmap.layout],
            None,
        );

        let pipeline = create_pipeline(device, targets, pipeline_layout);

        let (mvp_sets, mvp_buffers) = util::create_uniform_sets(
            device,
            mvp_layout,
            size_of::<HandUniform>() as u64,
            "hand_uniform",
        );

        let (skin, skin_w, skin_h) = load_skin_texture(device, jar_assets_dir, asset_index);

        let skin_sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);

        let skin_set = util::create_texture_group(device, skin_layout, skin.handle(), skin_sampler);

        // Wide arms until the profile's skin (and its model flag) is fetched.
        let (vertex_buffer, vertex_count) = create_arm_vertex_buffer(device, skin_w, skin_h, false);

        tracing::info!(
            "Hand pipeline initialized ({vertex_count} vertices, skin {skin_w}x{skin_h})"
        );

        Self {
            pipeline,
            pipeline_layout,
            mvp_layout,
            skin_layout,
            mvp_sets,
            skin_set,
            mvp_buffers,
            lightmap_set: lightmap.group,
            vertex_buffer,
            vertex_count,
            skin,
            skin_sampler,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_and_draw(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        aspect: f32,
        hud_fov: f32,
        swing_progress: f32,
        right_arm: bool,
        bob: Mat4,
        light: LightLevels,
    ) {
        let proj = projection(aspect, hud_fov);
        let model = arm_matrix(swing_progress, arm_invert(right_arm));

        let mvp = proj * bob * model;
        let uniform = HandUniform {
            mvp: mvp.to_cols_array_2d(),
            light: [light.lane(), 0.0, 0.0, 0.0],
        };
        let bytes = bytemuck::bytes_of(&uniform);
        self.mvp_buffers[frame].write(bytes);

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_groups(0, &[self.mvp_sets[frame], self.skin_set, self.lightmap_set]);
        pass.set_vertex_buffer(0, self.vertex_buffer.handle());
        let first_vertex = if right_arm { 0 } else { self.vertex_count };
        pass.draw(self.vertex_count, 1, first_vertex, 0);
    }

    pub fn skin_texture(&self) -> rhi::backend::TextureHandle {
        self.skin.handle()
    }

    pub fn skin_sampler(&self) -> rhi::backend::Sampler {
        self.skin_sampler
    }

    pub fn reload_skin(&mut self, device: &rhi::backend::Device, skin: &SkinData) {
        let texture = upload_skin_to_gpu(device, &skin.pixels, skin.width, skin.height);

        std::mem::replace(&mut self.skin, texture).destroy(device);
        self.skin_set.replace(
            device,
            self.skin_layout,
            &util::texture_binding(self.skin.handle(), self.skin_sampler),
        );

        let (vertex_buffer, vertex_count) =
            create_arm_vertex_buffer(device, skin.width, skin.height, skin.slim);
        std::mem::replace(&mut self.vertex_buffer, vertex_buffer).destroy(device);
        self.vertex_count = vertex_count;

        tracing::info!(
            "Skin reloaded: {}x{} (slim: {})",
            skin.width,
            skin.height,
            skin.slim
        );
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
        util::destroy_buffers(device, &mut self.mvp_buffers);
        std::mem::take(&mut self.vertex_buffer).destroy(device);

        self.skin_sampler.destroy(device);
        std::mem::take(&mut self.skin).destroy(device);

        self.pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.mvp_sets.drain(..));
        self.skin_set.destroy(device);
        self.mvp_layout.destroy(device);
        self.skin_layout.destroy(device);
    }
}

/// Vanilla `ItemInHandRenderer`'s `invert`: 1 for the right arm, -1 for the
/// left.
pub(super) fn arm_invert(right_arm: bool) -> f32 {
    if right_arm { 1.0 } else { -1.0 }
}

/// Vanilla `ItemInHandRenderer.renderPlayerArm`, mirrored by `invert`.
fn arm_matrix(swing_progress: f32, invert: f32) -> Mat4 {
    let sp = swing_progress;
    let sqrt_sp = sp.sqrt();
    let pi = std::f32::consts::PI;

    let x_off = -0.3 * (sqrt_sp * pi).sin();
    let y_off = 0.4 * (sqrt_sp * pi * 2.0).sin();
    let z_off = -0.4 * (sp * pi).sin();

    let swing_y = invert * (sqrt_sp * pi).sin() * 70.0_f32.to_radians();
    let swing_z = invert * (sp * sp * pi).sin() * (-20.0_f32).to_radians();

    // `PlayerModel`'s arm pivots at (-5, 2, 0) on the right and (5, 2, 0)
    // on the left; `renderHand` sets their zRot to 0.1 and -0.1.
    let pivot = Vec3::new(invert * -5.0 / 16.0, 2.0 / 16.0, 0.0);
    let arm_local_rot = Mat4::from_translation(pivot) * Mat4::from_rotation_z(invert * 0.1);

    Mat4::from_translation(Vec3::new(
        invert * (x_off + 0.64),
        y_off - 0.6,
        z_off - 0.72,
    )) * Mat4::from_rotation_y((invert * 45.0_f32).to_radians())
        * Mat4::from_rotation_y(swing_y)
        * Mat4::from_rotation_z(swing_z)
        * Mat4::from_translation(Vec3::new(-invert, 3.6, 3.5))
        * Mat4::from_rotation_z((invert * 120.0_f32).to_radians())
        * Mat4::from_rotation_x(200.0_f32.to_radians())
        * Mat4::from_rotation_y((invert * -135.0_f32).to_radians())
        * Mat4::from_translation(Vec3::new(invert * 5.6, 0.0, 0.0))
        * arm_local_rot
}

pub(super) fn projection(aspect: f32, hud_fov: f32) -> Mat4 {
    let mut proj = proj::directx::perspective(hud_fov, aspect, NEAR, FAR);
    proj.y_axis.y *= -1.0;
    proj
}

fn create_arm_vertex_buffer(
    device: &rhi::backend::Device,
    skin_w: u32,
    skin_h: u32,
    slim: bool,
) -> (rhi::backend::Buffer, u32) {
    // The right arm, then the left.
    let mut vertices = build_arm_vertices(skin_w, skin_h, slim, true);
    let arm_vertex_count = vertices.len() as u32;
    vertices.extend(build_arm_vertices(skin_w, skin_h, slim, false));
    let buffer = rhi::backend::Buffer::with_data(
        device,
        "hand_vertices",
        bytemuck::cast_slice(&vertices),
        rhi::BufferUsages::VERTEX,
    );
    (buffer, arm_vertex_count)
}

fn build_arm_vertices(skin_w: u32, skin_h: u32, slim: bool, right: bool) -> Vec<HandVertex> {
    let sw = skin_w as f32;
    let sh = skin_h as f32;

    // Vanilla right arm addBox(-3, -2, -2, 4, 12, 4), or the slim layout's
    // addBox(-2, -2, -2, 3, 12, 4); the left arm's starts at x -1 either way.
    // Scaled to blocks (1/16). Model space is Y-down: y0 is the shoulder
    // end, y1 the hand end.
    let w = if slim { 3.0 } else { 4.0 };
    let x0: f32 = match (right, slim) {
        (false, _) => -1.0,
        (true, true) => -2.0,
        (true, false) => -3.0,
    } / 16.0;
    let x1: f32 = x0 + w / 16.0;
    let y0: f32 = -2.0 / 16.0;
    let y1: f32 = 10.0 / 16.0;
    let z0: f32 = -2.0 / 16.0;
    let z1: f32 = 2.0 / 16.0;

    // texOffs(40, 16) on the right, (32, 48) on the left; h=12 d=4
    let (u0, v0) = if right { (40.0, 16.0) } else { (32.0, 48.0) };
    let h = 12.0;
    let d = 4.0;

    let u1 = u0 + d;
    let u2 = u1 + w;
    let u3 = u2 + d;
    let u4 = u3 + w;
    let v1 = v0 + d;
    let v2 = v1 + h;

    // Corner naming follows vanilla ModelPart.Cube (t* = min Z, l* = max Z)
    let t0 = [x0, y0, z0];
    let t1 = [x1, y0, z0];
    let t2 = [x1, y1, z0];
    let t3 = [x0, y1, z0];
    let l0 = [x0, y0, z1];
    let l1 = [x1, y0, z1];
    let l2 = [x1, y1, z1];
    let l3 = [x0, y1, z1];

    let mut verts = Vec::with_capacity(36);

    // UV corner order matches vanilla ModelPart.Polygon
    let mut quad = |positions: [[f32; 3]; 4], uv_px: [f32; 4]| {
        let ua = uv_px[0] / sw;
        let va = uv_px[1] / sh;
        let ub = uv_px[2] / sw;
        let vb = uv_px[3] / sh;
        let uvs = [[ub, va], [ua, va], [ua, vb], [ub, vb]];
        for &i in &[0usize, 1, 2, 0, 2, 3] {
            verts.push(HandVertex {
                position: positions[i],
                uv: uvs[i],
            });
        }
    };

    quad([l1, l0, t0, t1], [u1, v0, u2, v1]); // down (shoulder cap)
    quad([t2, t3, l3, l2], [u2, v1, u2 + w, v0]); // up (hand cap, V reversed as vanilla)
    quad([t0, l0, l3, t3], [u0, v1, u1, v2]); // west (outer)
    quad([t1, t0, t3, t2], [u1, v1, u2, v2]); // north (front)
    quad([l1, t1, t2, l2], [u2, v1, u3, v2]); // east (inner)
    quad([l0, l1, l2, l3], [u3, v1, u4, v2]); // south (back)

    verts
}

fn load_skin_texture(
    device: &rhi::backend::Device,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) -> (rhi::backend::Texture, u32, u32) {
    let skin_key = "minecraft/textures/entity/player/wide/steve.png";
    let skin_path = resolve_asset_path(jar_assets_dir, asset_index, skin_key);

    let (pixels, width, height) = util::load_png(&skin_path).unwrap_or_else(|| {
        tracing::warn!(
            "Failed to load skin from {}, using fallback",
            skin_path.display()
        );
        fallback_skin()
    });

    let texture = upload_skin_to_gpu(device, &pixels, width, height);
    (texture, width, height)
}

fn upload_skin_to_gpu(
    device: &rhi::backend::Device,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> rhi::backend::Texture {
    util::create_sampled_texture(device, "skin", pixels, width, height)
}

fn fallback_skin() -> (Vec<u8>, u32, u32) {
    let w = 64u32;
    let h = 64u32;
    let pixels = [196u8, 161, 125, 255].repeat((w * h) as usize);
    (pixels, w, h)
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
                compare: rhi::CompareFunction::Less,
                ..rhi::DepthState::WRITE
            }),
            ..rhi::RenderPipelineDesc::new(
                "hand",
                shader::include_shader!("hand.vert"),
                shader::include_shader!("hand.frag"),
                &[rhi::vertex_buffer!(HandVertex, [Float32x3, Float32x2])],
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_left_arm_mirrors_the_right() {
        let flip = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        for swing in [0.0, 0.3, 0.7, 1.0] {
            let mirrored = flip * arm_matrix(swing, 1.0) * flip;
            assert!(
                arm_matrix(swing, -1.0).abs_diff_eq(mirrored, 1e-4),
                "swing {swing}"
            );
        }
        for slim in [false, true] {
            let x_range = |right| {
                let verts = build_arm_vertices(64, 64, slim, right);
                let xs = verts.iter().map(|v| v.position[0]);
                (
                    xs.clone().fold(f32::MAX, f32::min),
                    xs.fold(f32::MIN, f32::max),
                )
            };
            let (right_min, right_max) = x_range(true);
            assert_eq!(x_range(false), (-right_max, -right_min), "slim {slim}");
        }
    }
}
