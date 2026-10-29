use std::f32::consts::TAU;
use std::path::Path;

use glam::{Mat4, Vec3, Vec4};

use crate::assets::AssetIndex;
use crate::renderer::camera::Camera;
use crate::renderer::{rhi, shader, util};
use crate::world::block::model::CardinalLightType;
use crate::world::environment::{EnvAttributes, Skybox};

const STAR_COUNT: u32 = 1500;
const SUN_SIZE: f32 = 30.0;
const MOON_SIZE: f32 = 20.0;
const CELESTIAL_DIST: f32 = 100.0;
const SKY_DISC_RADIUS: f32 = 512.0;
const SUNRISE_STEPS: u32 = 16;
const END_SKY_SIZE: f32 = 100.0;
/// The end sky's texture repeats this many times across a face.
const END_SKY_REPEATS: f32 = 16.0;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SkyVertex {
    position: [f32; 3],
    uv: [f32; 2],
}

impl SkyVertex {
    pub const BUFFER: rhi::VertexBufferLayout<'static> =
        rhi::vertex_buffer!(Self, [Float32x3, Float32x2]);
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SkyUniform {
    view_proj: [[f32; 4]; 4],
    /// rgb: the fog colour (sRGB), w: vanilla's `FogSkyEnd`.
    fog_color: [f32; 4],
}

/// What a sky draw is: Iris's render stage for it (`MC_RENDER_STAGE_*`),
/// which `sky.frag` branches on too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum SkyStage {
    Sky = 1,
    Sunset = 2,
    CustomSky = 3,
    Sun = 4,
    Moon = 5,
    Stars = 6,
    Void = 7,
}

impl SkyStage {
    /// Whether a pack draws it with `gbuffers_skytextured` rather than
    /// `gbuffers_skybasic`.
    pub fn textured(self) -> bool {
        matches!(self, Self::CustomSky | Self::Sun | Self::Moon)
    }

    /// Whether it adds onto the sky, vanilla's `OVERLAY` blend.
    pub fn overlay(self) -> bool {
        matches!(self, Self::Sun | Self::Moon | Self::Stars)
    }
}

/// The push block `sky.vert` and a pack's sky prelude read.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SkyPush {
    /// Vanilla's pose for the element, on top of the sky's view.
    pose: [[f32; 4]; 4],
    /// Vanilla's colour modulator for it.
    color: [f32; 4],
    stage: u32,
}

pub const SKY_PUSH_SIZE: u32 = size_of::<SkyPush>() as u32;

/// One element of the sky, as vanilla's `SkyRenderer.render` draws it.
#[derive(Clone, Copy)]
pub struct SkyDraw {
    pub stage: SkyStage,
    push: SkyPush,
    texture: rhi::backend::BindGroup,
    vertices: (u32, u32),
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
    pub skybox: Skybox,
    /// `FogRenderer`'s void darkness at the camera
    /// ([`crate::renderer::fog::void_darkness`]).
    pub void_darkness: f32,
    /// Vanilla `SkyRenderer.shouldRenderDarkDisc`: the eyes are below the
    /// horizon and out of water.
    pub dark_disc: bool,
    /// The frame's fog colour (sRGB), which the renderer computes from the
    /// rest ([`crate::renderer::fog::fog_color`]).
    pub fog_color: Vec3,
    /// The dimension's directional light, vanilla `Lighting.updateLevel`.
    pub cardinal_light: CardinalLightType,
}

impl SkyState {
    pub fn default_day() -> Self {
        Self {
            day_time: 6000,
            game_time: 6000,
            rain_level: 0.0,
            thunder_level: 0.0,
            partial_tick: 0.0,
            attributes: EnvAttributes::default(),
            skybox: Skybox::Overworld,
            void_darkness: 0.0,
            dark_disc: false,
            fog_color: Vec3::ZERO,
            cardinal_light: CardinalLightType::Default,
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
    end_sky_set: rhi::backend::BindGroup,
    ubo_buffers: Vec<rhi::backend::Buffer>,
    vertex_buffer: rhi::backend::Buffer,
    ranges: Ranges,
    sun: rhi::backend::Texture,
    sampler: rhi::backend::Sampler,
    moons: Vec<rhi::backend::Texture>,
    end_sky: rhi::backend::Texture,
    end_sky_sampler: rhi::backend::Sampler,
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
                size: SKY_PUSH_SIZE,
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

        let end_sky = util::load_asset_texture(
            device,
            jar_assets_dir,
            asset_index,
            "minecraft/textures/environment/end_sky.png",
        );
        let end_sky_sampler =
            rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST_REPEAT);
        let end_sky_set =
            util::create_texture_group(device, tex_layout, end_sky.handle(), end_sky_sampler);

        let (vertices, ranges) = build_all_geometry();
        let vertex_buffer = rhi::backend::Buffer::with_data(
            device,
            "sky_vertices",
            bytemuck::cast_slice(&vertices),
            rhi::BufferUsages::VERTEX,
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
            end_sky_set,
            ubo_buffers,
            vertex_buffer,
            ranges,
            sun,
            sampler,
            moons,
            end_sky,
            end_sky_sampler,
        }
    }

    pub fn update_and_draw(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        camera: &Camera,
        sky: &SkyState,
        render_distance: u32,
    ) {
        // `AtmosphericFogEnvironment.setupFog`'s sky end.
        let sky_fog_end = ((render_distance * 16) as f32).min(sky.attributes.sky_fog_end_distance);
        let uniform = SkyUniform {
            view_proj: camera.sky_view_projection().to_cols_array_2d(),
            fog_color: sky.fog_color.extend(sky_fog_end).to_array(),
        };
        self.ubo_buffers[frame].write(bytemuck::bytes_of(&uniform));

        let mut overlay = None;
        for draw in self.draws(camera, sky, 0.0) {
            let blend = draw.stage.overlay();
            if overlay != Some(blend) {
                overlay = Some(blend);
                pass.set_pipeline(if blend {
                    &self.overlay_pipeline
                } else {
                    &self.pipeline
                });
                pass.set_bind_group(0, self.ubo_sets[frame]);
            }
            self.record(pass, &draw, 1);
        }
    }

    /// [`elements`] with their textures and vertices; `tilt` is a shader
    /// pack's `sunPathRotation`.
    pub fn draws(&self, camera: &Camera, sky: &SkyState, tilt: f32) -> Vec<SkyDraw> {
        elements(sky, camera.top_down().is_some(), tilt)
            .into_iter()
            .map(|(stage, pose, color)| {
                let (texture, vertices) = match stage {
                    SkyStage::Sky => (self.sun_set, self.ranges.top_disc),
                    SkyStage::Sunset => (self.sun_set, self.ranges.sunrise),
                    SkyStage::CustomSky => (self.end_sky_set, self.ranges.end_sky),
                    SkyStage::Sun => (self.sun_set, self.ranges.sun),
                    SkyStage::Moon => (
                        self.moon_sets[usize::from(sky.attributes.moon_phase % 8)],
                        self.ranges.moon,
                    ),
                    SkyStage::Stars => (self.sun_set, self.ranges.stars),
                    SkyStage::Void => (self.sun_set, self.ranges.dark_disc),
                };
                SkyDraw {
                    stage,
                    push: SkyPush {
                        pose: pose.to_cols_array_2d(),
                        color: color.to_array(),
                        stage: stage as u32,
                    },
                    texture,
                    vertices,
                }
            })
            .collect()
    }

    /// Records `draw` into a pass whose pipeline layout takes the sky push
    /// block, its texture bound at `texture_set`.
    pub fn record(&self, pass: &rhi::backend::RenderPassEncoder, draw: &SkyDraw, texture_set: u32) {
        pass.set_vertex_buffer(0, self.vertex_buffer.handle());
        pass.set_bind_group(texture_set, draw.texture);
        pass.set_push_constants(
            rhi::ShaderStages::VERTEX_FRAGMENT,
            0,
            bytemuck::bytes_of(&draw.push),
        );
        let (first, count) = draw.vertices;
        pass.draw(count, 1, first, 0);
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
        self.end_sky_sampler.destroy(device);
        std::mem::take(&mut self.sun).destroy(device);
        std::mem::take(&mut self.end_sky).destroy(device);

        for moon in self.moons.drain(..) {
            moon.destroy(device);
        }

        self.pipeline.destroy(device);
        self.overlay_pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.ubo_sets.drain(..));
        self.sun_set.destroy(device);
        util::destroy_bind_groups(device, self.moon_sets);
        self.end_sky_set.destroy(device);
        self.ubo_layout.destroy(device);
        self.tex_layout.destroy(device);
    }
}

/// Vanilla's `rotateDegrees(YP, -90)` then `rotate(XP, angle)` for the sun,
/// moon and stars, with Iris's `sunPathRotation` `tilt` about Z between.
pub fn celestial_pose(degrees: f32, tilt: f32) -> Mat4 {
    Mat4::from_rotation_y(-90f32.to_radians())
        * Mat4::from_rotation_z(tilt.to_radians())
        * Mat4::from_rotation_x(degrees.to_radians())
}

/// Vanilla `SkyRenderer.render`'s elements in order, each with its pose and
/// colour modulator. The chunk-load benchmark's `top_down` view keeps the
/// sky backdrop but drops the sun, moon, stars and sunrise glow so only
/// terrain reads.
fn elements(sky: &SkyState, top_down: bool, tilt: f32) -> Vec<(SkyStage, Mat4, Vec4)> {
    let attributes = &sky.attributes;
    match sky.skybox {
        Skybox::None => return Vec::new(),
        // Vanilla `SkyRenderer.renderEndSky`: grey 0x28.
        // TODO: the end flash.
        Skybox::End => {
            let grey = Vec3::splat(40.0 / 255.0).extend(1.0);
            return vec![(SkyStage::CustomSky, Mat4::IDENTITY, grey)];
        }
        Skybox::Overworld => {}
    }
    let mut elements = vec![(
        SkyStage::Sky,
        Mat4::IDENTITY,
        attributes.sky_color.extend(1.0),
    )];
    if top_down {
        return elements;
    }

    let sunrise = attributes.sunrise_sunset_color;
    if sunrise.w > 0.001 {
        let flip = if attributes.sun_angle.to_radians().sin() < 0.0 {
            180.0
        } else {
            0.0
        };
        let pose = Mat4::from_rotation_x(90f32.to_radians())
            * Mat4::from_rotation_z((flip + 90f32).to_radians())
            * Mat4::from_scale(Vec3::new(1.0, 1.0, sunrise.w));
        elements.push((SkyStage::Sunset, pose, sunrise));
    }

    let celestial = |degrees| celestial_pose(degrees, tilt);
    let rain_brightness = Vec4::new(1.0, 1.0, 1.0, 1.0 - sky.rain());
    elements.push((
        SkyStage::Sun,
        celestial(attributes.sun_angle),
        rain_brightness,
    ));
    elements.push((
        SkyStage::Moon,
        celestial(attributes.moon_angle),
        rain_brightness,
    ));
    if attributes.star_brightness > 0.0 {
        elements.push((
            SkyStage::Stars,
            celestial(attributes.star_angle),
            Vec4::splat(attributes.star_brightness),
        ));
    }
    if sky.dark_disc {
        elements.push((
            SkyStage::Void,
            Mat4::from_translation(Vec3::new(0.0, 12.0, 0.0)),
            Vec4::W,
        ));
    }
    elements
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

/// Each element's first vertex and vertex count in `sky_vertices`.
struct Ranges {
    top_disc: (u32, u32),
    stars: (u32, u32),
    sun: (u32, u32),
    moon: (u32, u32),
    sunrise: (u32, u32),
    dark_disc: (u32, u32),
    end_sky: (u32, u32),
}

fn build_all_geometry() -> (Vec<SkyVertex>, Ranges) {
    let mut verts = Vec::new();
    let mut add = |build: &dyn Fn(&mut Vec<SkyVertex>)| {
        let first = verts.len() as u32;
        build(&mut verts);
        (first, verts.len() as u32 - first)
    };
    let ranges = Ranges {
        top_disc: add(&|v| build_sky_disc(v, 16.0)),
        stars: add(&build_stars),
        sun: add(&|v| build_celestial_quad(v, SUN_SIZE, CELESTIAL_UVS)),
        // Each phase is its own texture, its quad's uvs turned half a turn.
        moon: add(&|v| {
            build_celestial_quad(v, MOON_SIZE, CELESTIAL_UVS.map(|[u, v]| [1.0 - u, 1.0 - v]))
        }),
        sunrise: add(&build_sunrise_fan),
        // Built at -16 and drawn 12 higher, so its fog distances are vanilla's.
        dark_disc: add(&|v| build_sky_disc(v, -16.0)),
        end_sky: add(&build_end_sky),
    };
    (verts, ranges)
}

/// Vanilla `SkyRenderer.buildEndSky`: the inside of a cube, one face turned
/// into each side.
fn build_end_sky(verts: &mut Vec<SkyVertex>) {
    use std::f32::consts::{FRAC_PI_2, PI};

    let faces = [
        glam::Mat3::IDENTITY,
        glam::Mat3::from_rotation_x(FRAC_PI_2),
        glam::Mat3::from_rotation_x(-FRAC_PI_2),
        glam::Mat3::from_rotation_x(PI),
        glam::Mat3::from_rotation_z(FRAC_PI_2),
        glam::Mat3::from_rotation_z(-FRAC_PI_2),
    ];
    let (s, r) = (END_SKY_SIZE, END_SKY_REPEATS);
    let corners = [
        ([-s, -s, -s], [0.0, 0.0]),
        ([-s, -s, s], [0.0, r]),
        ([s, -s, s], [r, r]),
        ([s, -s, -s], [r, 0.0]),
    ];
    for face in faces {
        for i in [0, 1, 2, 0, 2, 3] {
            let (position, uv) = corners[i];
            verts.push(SkyVertex {
                position: (face * Vec3::from_array(position)).to_array(),
                uv,
            });
        }
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

const CELESTIAL_UVS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

fn build_celestial_quad(verts: &mut Vec<SkyVertex>, size: f32, uvs: [[f32; 2]; 4]) {
    let corners = [
        [-size, CELESTIAL_DIST, -size],
        [size, CELESTIAL_DIST, -size],
        [size, CELESTIAL_DIST, size],
        [-size, CELESTIAL_DIST, size],
    ];

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
                // Clipping the disc where it passes behind the eye can open a
                // pixel-wide crack along the edges its triangles share.
                depth_clamp: true,
                ..rhi::RenderPipelineDesc::new(
                    label,
                    shader::include_shader!("sky.vert"),
                    shader::include_shader!("sky.frag"),
                    &[SkyVertex::BUFFER],
                )
            },
        )
    };
    (
        create("sky", rhi::BlendState::TRANSLUCENT),
        create("sky overlay", rhi::BlendState::OVERLAY),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stages(sky: &SkyState, top_down: bool) -> Vec<SkyStage> {
        elements(sky, top_down, 0.0)
            .into_iter()
            .map(|(s, _, _)| s)
            .collect()
    }

    #[test]
    fn elements_follow_vanilla_order_and_conditions() {
        let mut sky = SkyState::default_day();
        sky.attributes.star_brightness = 0.0;
        sky.attributes.sunrise_sunset_color = Vec4::ZERO;
        assert_eq!(
            stages(&sky, false),
            [SkyStage::Sky, SkyStage::Sun, SkyStage::Moon]
        );
        assert_eq!(stages(&sky, true), [SkyStage::Sky]);

        sky.attributes.star_brightness = 0.5;
        sky.attributes.sunrise_sunset_color = Vec4::new(1.0, 0.5, 0.2, 0.8);
        sky.dark_disc = true;
        assert_eq!(
            stages(&sky, false),
            [
                SkyStage::Sky,
                SkyStage::Sunset,
                SkyStage::Sun,
                SkyStage::Moon,
                SkyStage::Stars,
                SkyStage::Void,
            ]
        );

        sky.skybox = Skybox::End;
        assert_eq!(stages(&sky, false), [SkyStage::CustomSky]);
        sky.skybox = Skybox::None;
        assert!(stages(&sky, false).is_empty());
    }

    #[test]
    fn sun_path_rotation_tilts_the_sun() {
        let sun = celestial_pose(0.0, 30.0).transform_point3(Vec3::new(0.0, 100.0, 0.0));
        let (s, c) = 30f32.to_radians().sin_cos();
        assert!(
            sun.abs_diff_eq(Vec3::new(0.0, 100.0 * c, -100.0 * s), 1e-3),
            "{sun}"
        );
    }

    #[test]
    fn stages_are_iris_render_stages() {
        use crate::shaderpack::preprocess::render_stage;
        for (stage, name) in [
            (SkyStage::Sky, "SKY"),
            (SkyStage::Sunset, "SUNSET"),
            (SkyStage::CustomSky, "CUSTOM_SKY"),
            (SkyStage::Sun, "SUN"),
            (SkyStage::Moon, "MOON"),
            (SkyStage::Stars, "STARS"),
            (SkyStage::Void, "VOID"),
        ] {
            assert_eq!(stage as usize, render_stage(name));
        }
    }

    #[test]
    fn poses_match_vanilla_pose_stacks() {
        let mut sky = SkyState::default_day();
        sky.attributes.sun_angle = 30.0;
        sky.attributes.sunrise_sunset_color = Vec4::new(1.0, 0.5, 0.2, 0.5);
        let elements = elements(&sky, false, 0.0);
        let pose = |stage| elements.iter().find(|e| e.0 == stage).unwrap().1;
        let close = |a: Vec3, b: Vec3| assert!(a.abs_diff_eq(b, 1e-3), "{a} != {b}");

        // `rotateDegrees(YP, -90)` then `rotate(XP, angle)`: the sun, overhead
        // at angle 0, swings toward -x as the angle grows.
        let sun = pose(SkyStage::Sun);
        let (s, c) = 30f32.to_radians().sin_cos();
        close(
            sun.transform_point3(Vec3::new(0.0, 100.0, 0.0)),
            Vec3::new(-100.0 * s, 100.0 * c, 0.0),
        );

        // `rotateDegrees(XP, 90)`, `rotateDegrees(ZP, 90)`, `scale(1, 1, a)`:
        // the fan's centre lies on the horizon, its depth squashed by alpha.
        let sunset = pose(SkyStage::Sunset);
        close(
            sunset.transform_point3(Vec3::new(0.0, 100.0, 0.0)),
            Vec3::new(-100.0, 0.0, 0.0),
        );
        close(
            sunset.transform_point3(Vec3::new(0.0, 0.0, -40.0)),
            Vec3::new(0.0, 20.0, 0.0),
        );
    }
}
