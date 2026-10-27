use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::path::Path;

use glam::{FloatExt, Vec3};

use crate::assets::AssetIndex;
use crate::renderer::camera::Camera;
use crate::renderer::{rhi, shader, util};

const STAR_COUNT: u32 = 1500;
const SUN_SIZE: f32 = 30.0;
const MOON_SIZE: f32 = 20.0;
const CELESTIAL_DIST: f32 = 100.0;
const SKY_DISC_RADIUS: f32 = 512.0;
const TICKS_PER_DAY: f32 = 24000.0;
const SUNRISE_STEPS: u32 = 16;

const MOON_BRIGHTNESS_PER_PHASE: [f32; 8] = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75];

const STAR_BRIGHTNESS_KEYFRAMES: &[(f32, f32)] = &[
    (92.0, 0.037),
    (627.0, 0.0),
    (11373.0, 0.0),
    (11732.0, 0.016),
    (11959.0, 0.044),
    (12399.0, 0.143),
    (12729.0, 0.258),
    (13228.0, 0.5),
    (22772.0, 0.5),
    (23032.0, 0.364),
    (23356.0, 0.225),
    (23758.0, 0.101),
];

const SKY_COLOR_KEYFRAMES: &[(f32, [f32; 3])] = &[
    (133.0, [1.0, 1.0, 1.0]),
    (11867.0, [1.0, 1.0, 1.0]),
    (13670.0, [0.0, 0.0, 0.0]),
    (22330.0, [0.0, 0.0, 0.0]),
];

/// The overworld timeline's `SKY_LIGHT_LEVEL` multiplier.
const SKY_LIGHT_LEVEL_KEYFRAMES: &[(f32, f32)] = &[
    (133.0, 1.0),
    (11867.0, 1.0),
    (13670.0, 0.266_666_68),
    (22330.0, 0.266_666_68),
];

const SUNRISE_COLOR_KEYFRAMES: &[(f32, i32)] = &[
    (71.0, 1609540403),
    (310.0, 703969843),
    (565.0, 117167155),
    (730.0, 16770355),
    (11270.0, 16770355),
    (11397.0, 83679283),
    (11522.0, 268028723),
    (11690.0, 703969843),
    (11929.0, 1609540403),
    (12243.0, -1310226637),
    (12358.0, -857440717),
    (12512.0, -371166669),
    (12613.0, -153261261),
    (12732.0, -19242189),
    (12841.0, -19440589),
    (13035.0, -321760973),
    (13252.0, -1043577037),
    (13775.0, 918435635),
    (13888.0, 532362547),
    (14039.0, 163001139),
    (14192.0, 11744051),
    (21807.0, 11678515),
    (21961.0, 163001139),
    (22112.0, 532362547),
    (22225.0, 918435635),
    (22748.0, -1043577037),
    (22965.0, -321760973),
    (23159.0, -19440589),
    (23272.0, -19242189),
    (23488.0, -371166669),
    (23642.0, -857440717),
    (23757.0, -1310226637),
];

const BASE_SKY_COLOR: [f32; 3] = [0.478, 0.659, 1.0];

// EasingType.symmetricCubicBezier(0.362, 0.241)
const SKY_ANGLE_BEZIER: (f32, f32, f32, f32) = (0.362, 0.241, 0.638, 0.759);

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
    /// The day time as of the last tick, which frames blend from.
    pub prev_day_time: u64,
    pub game_time: u64,
    pub rain_level: f32,
    pub thunder_level: f32,
    pub partial_tick: f32,
}

impl SkyState {
    pub fn default_day() -> Self {
        Self {
            day_time: 6000,
            prev_day_time: 6000,
            game_time: 6000,
            rain_level: 0.0,
            thunder_level: 0.0,
            partial_tick: 0.0,
        }
    }

    /// The day's tick for this frame. Vanilla samples the sky's attributes at
    /// whole ticks (`AttributeTrackSampler`), then blends the last tick's into
    /// this one's by the partial tick (`EnvironmentAttributeProbe`), its angles
    /// snapping on a jump of 90 degrees or more (`LerpFunction.ofDegrees`).
    pub fn day_tick(&self) -> f32 {
        let (from, to) = (tick_of_day(self.prev_day_time), tick_of_day(self.day_time));
        let half_day = TICKS_PER_DAY / 2.0;
        let delta = (to - from + half_day).rem_euclid(TICKS_PER_DAY) - half_day;
        let turn = (sun_angle(to) - sun_angle(from) + PI).rem_euclid(TAU) - PI;
        if turn.abs() >= FRAC_PI_2 {
            return to;
        }
        (from + self.partial_tick * delta).rem_euclid(TICKS_PER_DAY)
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

    pub fn sky_color(&self) -> [f32; 3] {
        let base = self.day_color(SKY_COLOR_KEYFRAMES);
        self.apply_weather(
            base,
            |c| blend_to_gray(c, 0.6, 0.75),
            |c| blend_to_gray(c, 0.24, 0.94),
        )
    }

    /// Cloud tint (rgba): vanilla base is white at 0.8 alpha, darkened toward
    /// night and blended to grey under rain/thunder. The day/night curve reuses
    /// the sky-colour keyframes (1.0 by day, 0.0 at midnight) with a floor so
    /// clouds stay faintly lit at night.
    pub fn cloud_color(&self) -> [f32; 4] {
        let day = sample_rgb_keyframes(self.day_tick(), SKY_COLOR_KEYFRAMES, TICKS_PER_DAY)[0];
        // Vanilla's night cloud multiplier is (0.1, 0.1, 0.15) — faintly blue.
        let base = [0.1 + 0.9 * day, 0.1 + 0.9 * day, 0.15 + 0.85 * day];
        let rgb = self.apply_weather(
            base,
            |c| blend_to_gray(c, 0.24, 0.5),
            |c| blend_to_gray(c, 0.095, 0.94),
        );
        [rgb[0], rgb[1], rgb[2], 0.8]
    }

    fn day_color(&self, keyframes: &[(f32, [f32; 3])]) -> [f32; 3] {
        let mult = sample_rgb_keyframes(self.day_tick(), keyframes, TICKS_PER_DAY);
        [
            BASE_SKY_COLOR[0] * mult[0],
            BASE_SKY_COLOR[1] * mult[1],
            BASE_SKY_COLOR[2] * mult[2],
        ]
    }

    /// Layers the rain then thunder color modifiers over the base color, each
    /// scaled by its level (thunder is gated by rain), matching vanilla
    /// `WeatherAttributes`: the sky blends to grey, the fog multiplies its RGB.
    fn apply_weather(
        &self,
        rgb: [f32; 3],
        rain_mod: impl Fn([f32; 3]) -> [f32; 3],
        thunder_mod: impl Fn([f32; 3]) -> [f32; 3],
    ) -> [f32; 3] {
        self.weather_layers(rgb, rain_mod, thunder_mod, lerp_rgb)
    }

    /// Vanilla `WeatherAttributes.addLayer`: the rain-only share of the rain
    /// level, then the thunder level, each blending toward its modified value.
    fn weather_layers<T: Copy>(
        &self,
        mut value: T,
        rain_mod: impl Fn(T) -> T,
        thunder_mod: impl Fn(T) -> T,
        lerp: impl Fn(T, T, f32) -> T,
    ) -> T {
        let thunder = self.thunder();
        let rain_only = (self.rain() - thunder).max(0.0);
        if rain_only > 0.0 {
            value = lerp(value, rain_mod(value), rain_only);
        }
        if thunder > 0.0 {
            value = lerp(value, thunder_mod(value), thunder);
        }
        value
    }

    /// Vanilla `Level.skyDarken`: 15 less `SKY_LIGHT_LEVEL`, which the
    /// overworld day timeline takes from 15 by day to 4 at night, and rain
    /// and thunder pull toward 4. Sampled per whole tick, as vanilla caches
    /// it. The end has no day timeline; the nether has no sky light to dim.
    pub fn sky_darken(&self, overworld_day: bool) -> u8 {
        let toward_night = |alpha: f32| move |level: f32| level.lerp(4.0, alpha);
        let day = if overworld_day {
            sample_float_keyframes(
                tick_of_day(self.day_time),
                SKY_LIGHT_LEVEL_KEYFRAMES,
                TICKS_PER_DAY,
            )
        } else {
            1.0
        };
        let level = self.weather_layers(
            15.0 * day,
            toward_night(0.3125),
            toward_night(0.527_343_75),
            FloatExt::lerp,
        );
        (15.0 - level) as u8
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
        let day_tick = sky.day_tick();

        let sun_angle = sun_angle(day_tick);
        let moon_angle = sun_angle + PI;
        let star_angle = sun_angle;

        // Stars fade out as rain ramps up (vanilla forces star brightness to 0).
        let star_brightness =
            sample_float_keyframes(day_tick, STAR_BRIGHTNESS_KEYFRAMES, TICKS_PER_DAY)
                * (1.0 - sky.rain());

        let dome = sky.sky_color();
        let sky_color = [dome[0], dome[1], dome[2], 1.0];

        let sunrise_argb = sample_argb_keyframes(day_tick, SUNRISE_COLOR_KEYFRAMES, TICKS_PER_DAY);

        let moon_phase_idx = ((sky.game_time / TICKS_PER_DAY as u64) % 8) as usize;
        let moon_brightness = MOON_BRIGHTNESS_PER_PHASE[moon_phase_idx];

        let celestial_alpha = 1.0 - sky.rain();

        let view_proj = camera.sky_view_projection();

        let uniform = SkyUniform {
            view_proj: view_proj.to_cols_array_2d(),
            sky_color,
            sunrise_color: sunrise_argb,
            sun_angle,
            moon_angle,
            star_angle,
            star_brightness,
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

        if star_brightness > 0.01 {
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

fn tick_of_day(day_time: u64) -> f32 {
    (day_time % TICKS_PER_DAY as u64) as f32
}

/// The sun's angle at a day tick, in radians.
fn sun_angle(day_tick: f32) -> f32 {
    let t = (day_tick - 6000.0).rem_euclid(TICKS_PER_DAY) / TICKS_PER_DAY;
    let (x1, y1, x2, y2) = SKY_ANGLE_BEZIER;
    cubic_bezier_ease(t, x1, y1, x2, y2) * TAU
}

/// CSS-style cubic Bezier easing with P0=(0,0), P3=(1,1). Solves Bx(s)=x via 4
/// Newton-Raphson iterations (matches vanilla EasingType.CubicBezier) and
/// returns By(s).
fn cubic_bezier_ease(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let cx = 3.0 * x1;
    let bx = 3.0 * (x2 - x1) - cx;
    let ax = 1.0 - cx - bx;
    let cy = 3.0 * y1;
    let by = 3.0 * (y2 - y1) - cy;
    let ay = 1.0 - cy - by;

    let mut s = x;
    for _ in 0..4 {
        let d = (3.0 * ax * s + 2.0 * bx) * s + cx;
        if d.abs() < 1e-6 {
            break;
        }
        s -= (((ax * s + bx) * s + cx) * s - x) / d;
    }
    let s = s.clamp(0.0, 1.0);
    ((ay * s + by) * s + cy) * s
}

fn keyframe_segment(
    t: f32,
    count: usize,
    tick_at: impl Fn(usize) -> f32,
    period: f32,
) -> (usize, usize, f32) {
    let mut i = 0;
    while i < count && tick_at(i) <= t {
        i += 1;
    }

    if i == 0 {
        let span = tick_at(0) + period - tick_at(count - 1);
        (count - 1, 0, (t + period - tick_at(count - 1)) / span)
    } else if i == count {
        let span = tick_at(0) + period - tick_at(count - 1);
        (count - 1, 0, (t - tick_at(count - 1)) / span)
    } else {
        (
            i - 1,
            i,
            (t - tick_at(i - 1)) / (tick_at(i) - tick_at(i - 1)),
        )
    }
}

fn sample_float_keyframes(tick: f32, keyframes: &[(f32, f32)], period: f32) -> f32 {
    let (a, b, frac) = keyframe_segment(tick % period, keyframes.len(), |i| keyframes[i].0, period);
    keyframes[a].1 + (keyframes[b].1 - keyframes[a].1) * frac
}

fn sample_rgb_keyframes(tick: f32, keyframes: &[(f32, [f32; 3])], period: f32) -> [f32; 3] {
    let (a, b, frac) = keyframe_segment(tick % period, keyframes.len(), |i| keyframes[i].0, period);
    let (v0, v1) = (keyframes[a].1, keyframes[b].1);
    [
        v0[0] + (v1[0] - v0[0]) * frac,
        v0[1] + (v1[1] - v0[1]) * frac,
        v0[2] + (v1[2] - v0[2]) * frac,
    ]
}

fn lerp_rgb(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Vanilla `ColorModifier.BlendToGray`: greyscale the color, scale it by
/// `brightness`, then lerp the original toward that darkened grey by `factor`.
fn blend_to_gray(rgb: [f32; 3], brightness: f32, factor: f32) -> [f32; 3] {
    let luma = rgb[0] * 0.3 + rgb[1] * 0.59 + rgb[2] * 0.11;
    let grey = [luma * brightness, luma * brightness, luma * brightness];
    lerp_rgb(rgb, grey, factor)
}

fn argb_to_rgba(argb: i32) -> [f32; 4] {
    let u = argb as u32;
    [
        ((u >> 16) & 0xFF) as f32 / 255.0,
        ((u >> 8) & 0xFF) as f32 / 255.0,
        (u & 0xFF) as f32 / 255.0,
        ((u >> 24) & 0xFF) as f32 / 255.0,
    ]
}

fn sample_argb_keyframes(tick: f32, keyframes: &[(f32, i32)], period: f32) -> [f32; 4] {
    let (a, b, frac) = keyframe_segment(tick % period, keyframes.len(), |i| keyframes[i].0, period);
    let (c0, c1) = (argb_to_rgba(keyframes[a].1), argb_to_rgba(keyframes[b].1));
    [
        c0[0] + (c1[0] - c0[0]) * frac,
        c0[1] + (c1[1] - c0[1]) * frac,
        c0[2] + (c1[2] - c0[2]) * frac,
        c0[3] + (c1[3] - c0[3]) * frac,
    ]
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

#[cfg(test)]
mod tests {
    use super::SkyState;

    fn at(day_time: u64, rain_level: f32, thunder_level: f32) -> SkyState {
        SkyState {
            day_time,
            prev_day_time: day_time,
            game_time: day_time,
            rain_level,
            thunder_level,
            partial_tick: 0.0,
        }
    }

    fn blended(prev_day_time: u64, day_time: u64, partial_tick: f32) -> f32 {
        SkyState {
            prev_day_time,
            partial_tick,
            ..at(day_time, 0.0, 0.0)
        }
        .day_tick()
    }

    #[test]
    fn frames_blend_from_the_last_ticks_day_time() {
        assert_eq!(blended(6000, 6001, 0.25), 6000.25);
        assert_eq!(blended(6000, 6000, 0.75), 6000.0);
        // Across midnight, the short way round.
        assert_eq!(blended(23999, 24000, 0.5), 23999.5);
        // Sped up, still blended.
        assert_eq!(blended(6000, 6100, 0.5), 6050.0);
    }

    #[test]
    fn a_jump_of_a_quarter_turn_or_more_snaps() {
        assert_eq!(blended(6000, 13000, 0.5), 13000.0);
        assert_eq!(blended(6000, 18000, 0.5), 18000.0);
    }

    #[test]
    fn sky_darkens_at_night_and_in_weather() {
        assert_eq!(at(6000, 0.0, 0.0).sky_darken(true), 0);
        // 15 x 0.26666668 rounds to 4 in f32, as in vanilla.
        assert_eq!(at(18000, 0.0, 0.0).sky_darken(true), 11);
        assert_eq!(at(6000, 1.0, 0.0).sky_darken(true), 3);
        assert_eq!(at(6000, 1.0, 1.0).sky_darken(true), 5);
        // Halfway through dusk: 15 x 0.633.
        assert_eq!(at(12768, 0.0, 0.0).sky_darken(true), 5);
        // The end keeps its sky light through the night.
        assert_eq!(at(18000, 0.0, 0.0).sky_darken(false), 0);
    }
}
