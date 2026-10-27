use std::f32::consts::TAU;
use std::path::Path;

use glam::Vec3;

use crate::assets::AssetIndex;
use crate::renderer::camera::Camera;
use crate::renderer::{rhi, shader, util};
use crate::world::environment::{EnvAttributes, Environment};

const STAR_COUNT: u32 = 1500;
const SUN_SIZE: f32 = 30.0;
const MOON_SIZE: f32 = 20.0;
const CELESTIAL_DIST: f32 = 100.0;
const SKY_DISC_RADIUS: f32 = 512.0;
const SUNRISE_STEPS: u32 = 16;

const MOON_BRIGHTNESS_PER_PHASE: [f32; 8] = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75];

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SkyVertex {
    position: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SkyUniform {
    view_proj: [[f32; 4]; 4],
    sky_color: [f32; 4],
    sunrise_color: [f32; 4],
    sun_angle: f32,
    moon_angle: f32,
    star_angle: f32,
    star_brightness: f32,
    celestial_alpha: f32,
    moon_brightness: f32,
    moon_phase: f32,
    _pad: f32,
}

#[derive(Clone)]
pub struct SkyState {
    pub day_time: u64,
    pub game_time: u64,
    pub rain_level: f32,
    pub thunder_level: f32,
    pub partial_tick: f32,
    /// The environment attributes, blended for this frame.
    pub attributes: EnvAttributes,
}

impl SkyState {
    pub fn default_day() -> Self {
        Self {
            day_time: 6000,
            game_time: 6000,
            rain_level: 0.0,
            thunder_level: 0.0,
            partial_tick: 0.0,
            attributes: Environment::default().resolve(6000, 0.0, 0.0),
        }
    }

    /// Clamped rain level (server-driven). Vanilla `setRainLevel` stores prev
    /// == current, so no sub-tick interpolation is needed here.
    pub fn rain(&self) -> f32 {
        self.rain_level.clamp(0.0, 1.0)
    }

    /// Effective thunder, gated by rain (vanilla `getThunderLevel` multiplies
    /// by the rain level so there is no thunder without rain).
    pub fn thunder(&self) -> f32 {
        self.thunder_level.clamp(0.0, 1.0) * self.rain()
    }
}

pub struct SkyPipeline {
    pipeline: rhi::backend::RenderPipeline,
    overlay_pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    ubo_layout: rhi::backend::BindGroupLayout,
    tex_layout: rhi::backend::BindGroupLayout,
    ubo_sets: Vec<rhi::backend::BindGroup>,
    sun_set: rhi::backend::BindGroup,
    moon_sets: [rhi::backend::BindGroup; 8],
    ubo_buffers: Vec<rhi::backend::Buffer>,
    vertex_buffer: rhi::backend::Buffer,
    top_disc_offset: u32,
    top_disc_count: u32,
    star_offset: u32,
    star_count: u32,
    sun_offset: u32,
    moon_offset: u32,
    sunrise_offset: u32,
    sunrise_count: u32,
    _dark_disc_offset: u32,
    _dark_disc_count: u32,
    sun: rhi::backend::Texture,
    sampler: rhi::backend::Sampler,
    moons: Vec<rhi::backend::Texture>,
}

impl SkyPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let ubo_layout =
            rhi::backend::create_bind_group_layout(device, util::VERTEX_FRAGMENT_UNIFORM);
        let tex_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[ubo_layout, tex_layout],
            Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::VERTEX_FRAGMENT,
                size: 4,
            }),
        );

        let (pipeline, overlay_pipeline) = create_pipelines(device, targets, pipeline_layout);

        let (ubo_sets, ubo_buffers) = util::create_uniform_sets(
            device,
            ubo_layout,
            std::mem::size_of::<SkyUniform>() as u64,
            "sky_uniform",
        );

        let sun = util::load_asset_texture(
            device,
            jar_assets_dir,
            asset_index,
            "minecraft/textures/environment/celestial/sun.png",
        );
        let sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::LINEAR);
        let sun_set = util::create_texture_group(device, tex_layout, sun.handle(), sampler);

        let moon_phase_paths = [
            "minecraft/textures/environment/celestial/moon/full_moon.png",
            "minecraft/textures/environment/celestial/moon/waning_gibbous.png",
            "minecraft/textures/environment/celestial/moon/third_quarter.png",
            "minecraft/textures/environment/celestial/moon/waning_crescent.png",
            "minecraft/textures/environment/celestial/moon/new_moon.png",
            "minecraft/textures/environment/celestial/moon/waxing_crescent.png",
            "minecraft/textures/environment/celestial/moon/first_quarter.png",
            "minecraft/textures/environment/celestial/moon/waxing_gibbous.png",
        ];
        let mut moons = Vec::with_capacity(8);
        let mut moon_sets = [rhi::backend::BindGroup::default(); 8];
        for (i, &path) in moon_phase_paths.iter().enumerate() {
            let moon = util::load_asset_texture(device, jar_assets_dir, asset_index, path);
            moon_sets[i] = util::create_texture_group(device, tex_layout, moon.handle(), sampler);
            moons.push(moon);
        }

        let geom = build_all_geometry();
        let vertex_bytes = bytemuck::cast_slice::<SkyVertex, u8>(&geom.vertices);
        let vertex_buffer = rhi::backend::Buffer::with_data(
            device,
            "sky_vertices",
            vertex_bytes,
            rhi::BufferUsages::VERTEX,
        );

        tracing::info!(
            "Sky pipeline initialized ({} top_disc, {} star, 6 sun, 6 moon, {} sunrise, {} dark_disc vertices)",
            geom.top_disc_count,
            geom.star_count,
            geom.sunrise_count,
            geom.dark_disc_count
        );

        Self {
            pipeline,
            overlay_pipeline,
            pipeline_layout,
            ubo_layout,
            tex_layout,
            ubo_sets,
            sun_set,
            moon_sets,
            ubo_buffers,
            vertex_buffer,
            top_disc_offset: geom.top_disc_offset,
            top_disc_count: geom.top_disc_count,
            star_offset: geom.star_offset,
            star_count: geom.star_count,
            sun_offset: geom.sun_offset,
            moon_offset: geom.moon_offset,
            sunrise_offset: geom.sunrise_offset,
            sunrise_count: geom.sunrise_count,
            _dark_disc_offset: geom.dark_disc_offset,
            _dark_disc_count: geom.dark_disc_count,
            sun,
            sampler,
            moons,
        }
    }

    pub fn update_and_draw(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        camera: &Camera,
        sky: &SkyState,
    ) {
        let attributes = &sky.attributes;
        let sky_color = attributes.sky_color.extend(1.0).to_array();
        let sunrise_argb = attributes.sunrise_sunset_color.to_array();
        let moon_phase_idx = usize::from(attributes.moon_phase % 8);
        let moon_brightness = MOON_BRIGHTNESS_PER_PHASE[moon_phase_idx];

        let celestial_alpha = 1.0 - sky.rain();

        let view_proj = camera.sky_view_projection();

        let uniform = SkyUniform {
            view_proj: view_proj.to_cols_array_2d(),
            sky_color,
            sunrise_color: sunrise_argb,
            sun_angle: attributes.sun_angle.to_radians(),
            moon_angle: attributes.moon_angle.to_radians(),
            star_angle: attributes.star_angle.to_radians(),
            star_brightness: attributes.star_brightness,
            celestial_alpha,
            moon_brightness,
            moon_phase: moon_phase_idx as f32,
            _pad: 0.0,
        };

        let bytes = bytemuck::bytes_of(&uniform);
        self.ubo_buffers[frame].write(bytes);

        let push_mode = |pass: &rhi::backend::RenderPassEncoder, mode: u32| {
            pass.set_push_constants(
                rhi::ShaderStages::VERTEX_FRAGMENT,
                0,
                bytemuck::bytes_of(&mode),
            );
        };
        let bind = |pass: &mut rhi::backend::RenderPassEncoder,
                    pipeline: rhi::backend::RenderPipeline| {
            pass.set_pipeline(&pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.handle());
            pass.set_bind_groups(0, &[self.ubo_sets[frame], self.sun_set]);
        };

        bind(pass, self.pipeline);
        push_mode(pass, 0);
        pass.draw(self.top_disc_count, 1, self.top_disc_offset, 0);

        // The chunk-load benchmark's top-down view keeps the sky backdrop but drops
        // the sun, moon, stars, and sunrise glow so only terrain reads.
        if camera.top_down().is_some() {
            return;
        }

        if sunrise_argb[3] > 0.001 {
            push_mode(pass, 5);
            pass.draw(self.sunrise_count, 1, self.sunrise_offset, 0);
        }

        bind(pass, self.overlay_pipeline);
        push_mode(pass, 2);
        pass.draw(6, 1, self.sun_offset, 0);

        pass.set_bind_group(1, self.moon_sets[moon_phase_idx]);
        push_mode(pass, 3);
        pass.draw(6, 1, self.moon_offset, 0);

        if attributes.star_brightness > 0.01 {
            push_mode(pass, 1);
            pass.draw(self.star_count, 1, self.star_offset, 0);
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.pipeline.destroy(device);
        self.overlay_pipeline.destroy(device);
        let (p, o) = create_pipelines(device, targets, self.pipeline_layout);
        self.pipeline = p;
        self.overlay_pipeline = o;
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.ubo_buffers);
        std::mem::take(&mut self.vertex_buffer).destroy(device);

        self.sampler.destroy(device);
        std::mem::take(&mut self.sun).destroy(device);

        for moon in self.moons.drain(..) {
            moon.destroy(device);
        }

        self.pipeline.destroy(device);
        self.overlay_pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.ubo_sets.drain(..));
        self.sun_set.destroy(device);
        util::destroy_bind_groups(device, self.moon_sets);
        self.ubo_layout.destroy(device);
        self.tex_layout.destroy(device);
    }
}

struct JavaRandom {
    seed: u64,
}

impl JavaRandom {
    fn new(seed: i64) -> Self {
        Self {
            seed: (seed as u64 ^ 25214903917) & 0xFFFF_FFFF_FFFF,
        }
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(25214903917).wrapping_add(11)) & 0xFFFF_FFFF_FFFF;
        (self.seed >> (48 - bits)) as i32
    }

    fn next_float(&mut self) -> f32 {
        self.next(24) as f32 / (1u32 << 24) as f32
    }

    fn next_double(&mut self) -> f64 {
        let hi = self.next(26) as i64;
        let lo = self.next(27) as i64;
        ((hi << 27) + lo) as f64 / (1i64 << 53) as f64
    }
}

struct Geometry {
    vertices: Vec<SkyVertex>,
    top_disc_offset: u32,
    top_disc_count: u32,
    star_offset: u32,
    star_count: u32,
    sun_offset: u32,
    moon_offset: u32,
    sunrise_offset: u32,
    sunrise_count: u32,
    dark_disc_offset: u32,
    dark_disc_count: u32,
}

fn build_all_geometry() -> Geometry {
    let mut verts = Vec::new();

    let top_disc_offset = 0u32;
    build_sky_disc(&mut verts, 16.0);
    let top_disc_count = verts.len() as u32;

    let star_offset = verts.len() as u32;
    build_stars(&mut verts);
    let star_count = verts.len() as u32 - star_offset;

    let sun_offset = verts.len() as u32;
    build_celestial_quad(&mut verts, SUN_SIZE);

    let moon_offset = verts.len() as u32;
    build_celestial_quad(&mut verts, MOON_SIZE);

    let sunrise_offset = verts.len() as u32;
    build_sunrise_fan(&mut verts);
    let sunrise_count = verts.len() as u32 - sunrise_offset;

    let dark_disc_offset = verts.len() as u32;
    build_sky_disc(&mut verts, -4.0);
    let dark_disc_count = verts.len() as u32 - dark_disc_offset;

    Geometry {
        vertices: verts,
        top_disc_offset,
        top_disc_count,
        star_offset,
        star_count,
        sun_offset,
        moon_offset,
        sunrise_offset,
        sunrise_count,
        dark_disc_offset,
        dark_disc_count,
    }
}

fn build_sky_disc(verts: &mut Vec<SkyVertex>, y: f32) {
    let sign = y.signum();
    let uv = [0.0, 0.0];
    let center = [0.0, y, 0.0];

    let mut perimeter = Vec::with_capacity(9);
    for deg in (-180..=180).step_by(45) {
        let rad = (deg as f32).to_radians();
        perimeter.push([
            sign * SKY_DISC_RADIUS * rad.cos(),
            y,
            SKY_DISC_RADIUS * rad.sin(),
        ]);
    }

    for i in 0..perimeter.len() - 1 {
        verts.push(SkyVertex {
            position: center,
            uv,
        });
        verts.push(SkyVertex {
            position: perimeter[i],
            uv,
        });
        verts.push(SkyVertex {
            position: perimeter[i + 1],
            uv,
        });
    }
}

fn build_stars(verts: &mut Vec<SkyVertex>) {
    let mut rng = JavaRandom::new(10842);
    let uv = [0.0, 0.0];

    for _ in 0..STAR_COUNT {
        let x = rng.next_float() * 2.0 - 1.0;
        let y = rng.next_float() * 2.0 - 1.0;
        let z = rng.next_float() * 2.0 - 1.0;
        let size = 0.15 + rng.next_float() * 0.1;
        let dist_sq = x * x + y * y + z * z;

        if dist_sq <= 0.01 || dist_sq >= 1.0 {
            continue;
        }

        let inv_len = CELESTIAL_DIST / dist_sq.sqrt();
        let center = Vec3::new(x * inv_len, y * inv_len, z * inv_len);

        let rotation_angle = rng.next_double() as f32 * TAU;

        let neg_center = -center.normalize();
        let rot = rotation_matrix_towards(neg_center, Vec3::Y, -rotation_angle);

        let corners = [
            Vec3::new(size, -size, 0.0),
            Vec3::new(size, size, 0.0),
            Vec3::new(-size, size, 0.0),
            Vec3::new(-size, -size, 0.0),
        ];

        let transformed: Vec<Vec3> = corners
            .iter()
            .map(|c| {
                let rotated = rot * *c;
                rotated + center
            })
            .collect();

        for &idx in &[0usize, 1, 2, 0, 2, 3] {
            verts.push(SkyVertex {
                position: transformed[idx].into(),
                uv,
            });
        }
    }
}

fn rotation_matrix_towards(direction: Vec3, up: Vec3, z_rotation: f32) -> glam::Mat3 {
    let forward = direction.normalize();
    let right = up.cross(forward).normalize();
    let actual_up = forward.cross(right);

    let basis = glam::Mat3::from_cols(right, actual_up, forward);

    let cz = z_rotation.cos();
    let sz = z_rotation.sin();
    let z_rot = glam::Mat3::from_cols(
        Vec3::new(cz, sz, 0.0),
        Vec3::new(-sz, cz, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
    );

    basis * z_rot
}

fn build_celestial_quad(verts: &mut Vec<SkyVertex>, size: f32) {
    let corners = [
        [-size, CELESTIAL_DIST, -size],
        [size, CELESTIAL_DIST, -size],
        [size, CELESTIAL_DIST, size],
        [-size, CELESTIAL_DIST, size],
    ];
    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

    for &idx in &[0usize, 1, 2, 0, 2, 3] {
        verts.push(SkyVertex {
            position: corners[idx],
            uv: uvs[idx],
        });
    }
}

fn build_sunrise_fan(verts: &mut Vec<SkyVertex>) {
    let center = SkyVertex {
        position: [0.0, 100.0, 0.0],
        uv: [1.0, 0.0],
    };

    let mut ring = Vec::with_capacity(SUNRISE_STEPS as usize + 1);
    for i in 0..=SUNRISE_STEPS {
        let angle = i as f32 * TAU / SUNRISE_STEPS as f32;
        let sa = angle.sin();
        let ca = angle.cos();
        ring.push(SkyVertex {
            position: [sa * 120.0, ca * 120.0, -ca * 40.0],
            uv: [0.0, 0.0],
        });
    }

    for i in 0..ring.len() - 1 {
        verts.push(center);
        verts.push(ring[i]);
        verts.push(ring[i + 1]);
    }
}

fn create_pipelines(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> (rhi::backend::RenderPipeline, rhi::backend::RenderPipeline) {
    let create = |label, blend| {
        rhi::backend::create_render_pipeline(
            device,
            targets,
            layout,
            &rhi::RenderPipelineDesc {
                blend: Some(blend),
                ..rhi::RenderPipelineDesc::new(
                    label,
                    shader::include_shader!("sky.vert"),
                    shader::include_shader!("sky.frag"),
                    &[rhi::vertex_buffer!(SkyVertex, [Float32x3, Float32x2])],
                )
            },
        )
    };
    (
        create("sky", rhi::BlendState::TRANSLUCENT),
        create("sky overlay", rhi::BlendState::OVERLAY),
    )
}
