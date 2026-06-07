use std::f32::consts::{PI, TAU};
use std::path::Path;
use std::sync::{Arc, Mutex};

use glam::Vec3;
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::camera::Camera;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};

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

const FOG_COLOR_KEYFRAMES: &[(f32, [f32; 3])] = &[
    (133.0, [1.0, 1.0, 1.0]),
    (11867.0, [1.0, 1.0, 1.0]),
    (13670.0, [0.06, 0.06, 0.09]),
    (22330.0, [0.06, 0.06, 0.09]),
];

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
    pub game_time: u64,
    pub rain_level: f32,
    pub thunder_level: f32,
    pub partial_tick: f32,
}

impl SkyState {
    pub fn default_day() -> Self {
        Self {
            day_time: 6000,
            game_time: 6000,
            rain_level: 0.0,
            thunder_level: 0.0,
            partial_tick: 0.0,
        }
    }

    pub fn day_tick(&self) -> f32 {
        (self.day_time % TICKS_PER_DAY as u64) as f32 + self.partial_tick
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

    pub fn fog_color(&self) -> [f32; 3] {
        let base = self.day_color(FOG_COLOR_KEYFRAMES);
        self.apply_weather(
            base,
            |c| mul_rgb(c, [0.5, 0.5, 0.6]),
            |c| mul_rgb(c, [0.25, 0.25, 0.3]),
        )
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
        mut rgb: [f32; 3],
        rain_mod: impl Fn([f32; 3]) -> [f32; 3],
        thunder_mod: impl Fn([f32; 3]) -> [f32; 3],
    ) -> [f32; 3] {
        let thunder = self.thunder();
        let rain_only = (self.rain() - thunder).max(0.0);
        if rain_only > 0.0 {
            rgb = lerp_rgb(rgb, rain_mod(rgb), rain_only);
        }
        if thunder > 0.0 {
            rgb = lerp_rgb(rgb, thunder_mod(rgb), thunder);
        }
        rgb
    }
}

pub struct SkyPipeline {
    pipeline: vk::Pipeline,
    overlay_pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    ubo_layout: vk::DescriptorSetLayout,
    tex_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    ubo_sets: Vec<vk::DescriptorSet>,
    sun_set: vk::DescriptorSet,
    moon_sets: [vk::DescriptorSet; 8],
    ubo_buffers: Vec<vk::Buffer>,
    ubo_allocations: Vec<Allocation>,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Allocation,
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
    sun_image: vk::Image,
    sun_view: vk::ImageView,
    sun_sampler: vk::Sampler,
    sun_allocation: Allocation,
    moon_images: [vk::Image; 8],
    moon_views: [vk::ImageView; 8],
    moon_sampler: vk::Sampler,
    moon_allocations: Vec<Allocation>,
}

impl SkyPipeline {
    pub fn new(
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let ubo_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
        );
        let tex_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::CombinedImageSampler,
            vk::ShaderStageFlags::Fragment,
        );

        let push_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
            offset: 0,
            size: 4,
        };
        let layouts = [ubo_layout, tex_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            push_constant_range_count: 1,
            push_constant_ranges: &push_range,
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create sky pipeline layout");

        let (pipeline, overlay_pipeline) = create_pipelines(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 9, // 1 sun + 8 moon phases
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: (MAX_FRAMES_IN_FLIGHT + 9) as u32,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create sky descriptor pool");

        let ubo_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| ubo_layout).collect();
        let ubo_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: ubo_layouts.len() as u32,
            set_layouts: ubo_layouts.as_ptr(),
            ..Default::default()
        };
        let mut ubo_sets = vec![vk::DescriptorSet::null(); ubo_layouts.len()];
        device
            .allocate_descriptor_sets(&ubo_alloc_info, &mut ubo_sets)
            .expect("failed to allocate sky ubo sets");

        let tex_layouts: Vec<_> = (0..9).map(|_| tex_layout).collect(); // 1 sun + 8 moon
        let tex_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: tex_layouts.len() as u32,
            set_layouts: tex_layouts.as_ptr(),
            ..Default::default()
        };
        let mut tex_sets = vec![vk::DescriptorSet::null(); tex_layouts.len()];
        device
            .allocate_descriptor_sets(&tex_alloc_info, &mut tex_sets)
            .expect("failed to allocate sky texture sets");
        let sun_set = tex_sets[0];
        let moon_sets: [vk::DescriptorSet; 8] = tex_sets[1..9].try_into().unwrap();

        let mut ubo_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut ubo_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &ubo_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<SkyUniform>() as u64,
                "sky_uniform",
            );
            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<SkyUniform>() as u64,
            };
            let write = vk::WriteDescriptorSet {
                dst_set: set,
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
                buffer_info: &buffer_info,
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);
            ubo_buffers.push(buf);
            ubo_allocations.push(alloc);
        }

        let (sun_image, sun_view, sun_allocation) = load_celestial_texture(
            device,
            queue,
            command_pool,
            allocator,
            jar_assets_dir,
            asset_index,
            "minecraft/textures/environment/celestial/sun.png",
        );
        let sun_sampler = unsafe { util::create_linear_sampler(device) };
        bind_texture_set(device, sun_set, sun_view, sun_sampler);

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
        let moon_sampler = unsafe { util::create_linear_sampler(device) };
        let mut moon_images = [vk::Image::null(); 8];
        let mut moon_views = [vk::ImageView::null(); 8];
        let mut moon_allocations = Vec::with_capacity(8);
        for (i, &path) in moon_phase_paths.iter().enumerate() {
            let (img, view, alloc) = load_celestial_texture(
                device,
                queue,
                command_pool,
                allocator,
                jar_assets_dir,
                asset_index,
                path,
            );
            moon_images[i] = img;
            moon_views[i] = view;
            moon_allocations.push(alloc);
            bind_texture_set(device, moon_sets[i], view, moon_sampler);
        }

        let geom = build_all_geometry();
        let vertex_bytes = bytemuck::cast_slice::<SkyVertex, u8>(&geom.vertices);
        let (vertex_buffer, vertex_allocation) = util::create_mapped_buffer(
            device,
            allocator,
            vertex_bytes,
            vk::BufferUsageFlags::VertexBuffer,
            "sky_vertices",
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
            descriptor_pool,
            ubo_sets,
            sun_set,
            moon_sets,
            ubo_buffers,
            ubo_allocations,
            vertex_buffer,
            vertex_allocation,
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
            sun_image,
            sun_view,
            sun_sampler,
            sun_allocation,
            moon_images,
            moon_views,
            moon_sampler,
            moon_allocations,
        }
    }

    pub fn update_and_draw(
        &mut self,
        device: &vk::Device,
        cmd: vk::CommandBuffer,
        frame: usize,
        camera: &Camera,
        sky: &SkyState,
    ) {
        let _ = device;
        let day_tick = sky.day_tick();

        let t = (day_tick - 6000.0).rem_euclid(TICKS_PER_DAY) / TICKS_PER_DAY;
        let (x1, y1, x2, y2) = SKY_ANGLE_BEZIER;
        let sun_angle = cubic_bezier_ease(t, x1, y1, x2, y2) * TAU;
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
        self.ubo_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);

        let push_stages = vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment;
        let push_mode = |cmd: vk::CommandBuffer, mode: u32| {
            cmd.push_constants(
                self.pipeline_layout,
                push_stages,
                0,
                bytemuck::bytes_of(&mode),
            );
        };

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_vertex_buffers(0, &[self.vertex_buffer], &[0]);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.ubo_sets[frame], self.sun_set],
            &[],
        );

        push_mode(cmd, 0);
        cmd.draw(self.top_disc_count, 1, self.top_disc_offset, 0);

        if sunrise_argb[3] > 0.001 {
            push_mode(cmd, 5);
            cmd.draw(self.sunrise_count, 1, self.sunrise_offset, 0);
        }

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.overlay_pipeline);
        cmd.bind_vertex_buffers(0, &[self.vertex_buffer], &[0]);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.ubo_sets[frame], self.sun_set],
            &[],
        );

        push_mode(cmd, 2);
        cmd.draw(6, 1, self.sun_offset, 0);

        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            1,
            &[self.moon_sets[moon_phase_idx]],
            &[],
        );
        push_mode(cmd, 3);
        cmd.draw(6, 1, self.moon_offset, 0);

        if star_brightness > 0.01 {
            push_mode(cmd, 1);
            cmd.draw(self.star_count, 1, self.star_offset, 0);
        }
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline(self.overlay_pipeline, None);
        let (p, o) = create_pipelines(device, render_pass, self.pipeline_layout);
        self.pipeline = p;
        self.overlay_pipeline = o;
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.ubo_buffers[i], None);
            alloc
                .free(std::mem::replace(&mut self.ubo_allocations[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }

        device.destroy_buffer(self.vertex_buffer, None);
        alloc
            .free(std::mem::replace(&mut self.vertex_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();

        let mut destroy_texture =
            |view: &mut vk::ImageView, image: &mut vk::Image, allocation: &mut Allocation| {
                device.destroy_image_view(*view, None);
                alloc
                    .free(std::mem::replace(allocation, unsafe { std::mem::zeroed() }))
                    .ok();
                device.destroy_image(*image, None);
            };

        device.destroy_sampler(self.sun_sampler, None);
        destroy_texture(
            &mut self.sun_view,
            &mut self.sun_image,
            &mut self.sun_allocation,
        );

        device.destroy_sampler(self.moon_sampler, None);
        for i in 0..8 {
            destroy_texture(
                &mut self.moon_views[i],
                &mut self.moon_images[i],
                &mut self.moon_allocations[i],
            );
        }

        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline(self.overlay_pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.ubo_layout, None);
        device.destroy_descriptor_set_layout(self.tex_layout, None);
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

fn mul_rgb(rgb: [f32; 3], m: [f32; 3]) -> [f32; 3] {
    [rgb[0] * m[0], rgb[1] * m[1], rgb[2] * m[2]]
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

fn load_celestial_texture(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    key: &str,
) -> (vk::Image, vk::ImageView, Allocation) {
    let path = resolve_asset_path(jar_assets_dir, asset_index, key);
    let (pixels, w, h) = util::load_png(&path).unwrap_or_else(|| {
        tracing::warn!("Failed to load {key}, using fallback");
        (vec![255u8; 16 * 16 * 4], 16, 16)
    });

    let (image, view, allocation) = util::create_gpu_image(device, allocator, w, h, key);
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, &pixels, &format!("{key}_staging"));

    util::upload_image(device, queue, command_pool, staging_buf, image, w, h);

    device.destroy_buffer(staging_buf, None);
    allocator.lock().unwrap().free(staging_alloc).ok();

    tracing::info!("Sky: loaded {key} ({w}x{h})");
    (image, view, allocation)
}

fn bind_texture_set(
    device: &vk::Device,
    set: vk::DescriptorSet,
    view: vk::ImageView,
    sampler: vk::Sampler,
) {
    let image_info = vk::DescriptorImageInfo {
        sampler,
        image_view: view,
        image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
    };
    let write = vk::WriteDescriptorSet {
        dst_set: set,
        dst_binding: 0,
        descriptor_type: vk::DescriptorType::CombinedImageSampler,
        descriptor_count: 1,
        image_info: &image_info,
        ..Default::default()
    };
    device.update_descriptor_sets(&[write], &[]);
}

fn create_pipelines(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> (vk::Pipeline, vk::Pipeline) {
    let vert_spv = shader::include_spirv!("sky.vert.spv");
    let frag_spv = shader::include_spirv!("sky.frag.spv");

    let vert_module = shader::create_shader_module(device, vert_spv);
    let frag_module = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Vertex,
            module: vert_module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Fragment,
            module: frag_module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
    ];

    let binding_descs = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: std::mem::size_of::<SkyVertex>() as u32,
        input_rate: vk::VertexInputRate::Vertex,
    }];

    let attr_descs = [
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R32G32B32Sfloat,
            offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R32G32Sfloat,
            offset: 12,
        },
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: binding_descs.len() as u32,
        vertex_binding_descriptions: binding_descs.as_ptr(),
        vertex_attribute_description_count: attr_descs.len() as u32,
        vertex_attribute_descriptions: attr_descs.as_ptr(),
        ..Default::default()
    };

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
        topology: vk::PrimitiveTopology::TriangleList,
        ..Default::default()
    };

    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewport_count: 1,
        scissor_count: 1,
        ..Default::default()
    };

    let rasterizer = vk::PipelineRasterizationStateCreateInfo {
        polygon_mode: vk::PolygonMode::Fill,
        cull_mode: vk::CullModeFlags::None,
        front_face: vk::FrontFace::CounterClockwise,
        line_width: 1.0,
        ..Default::default()
    };

    let multisampling = vk::PipelineMultisampleStateCreateInfo {
        rasterization_samples: vk::SampleCountFlags::Type1,
        ..Default::default()
    };

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo {
        depth_test_enable: vk::FALSE,
        depth_write_enable: vk::FALSE,
        ..Default::default()
    };

    let translucent_blend = vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::SrcAlpha,
        dst_color_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: vk::BlendFactor::One,
        dst_alpha_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        alpha_blend_op: vk::BlendOp::Add,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    };

    let overlay_blend = vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::SrcAlpha,
        dst_color_blend_factor: vk::BlendFactor::One,
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: vk::BlendFactor::One,
        dst_alpha_blend_factor: vk::BlendFactor::Zero,
        alpha_blend_op: vk::BlendOp::Add,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    };

    let dynamic_states = [vk::DynamicState::Viewport, vk::DynamicState::Scissor];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo {
        dynamic_state_count: dynamic_states.len() as u32,
        dynamic_states: dynamic_states.as_ptr(),
        ..Default::default()
    };

    let translucent_blending = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: 1,
        attachments: &translucent_blend,
        ..Default::default()
    };
    let overlay_blending = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: 1,
        attachments: &overlay_blend,
        ..Default::default()
    };

    let infos = [
        vk::GraphicsPipelineCreateInfo {
            stage_count: stages.len() as u32,
            stages: stages.as_ptr(),
            vertex_input_state: &vertex_input,
            input_assembly_state: &input_assembly,
            viewport_state: &viewport_state,
            rasterization_state: &rasterizer,
            multisample_state: &multisampling,
            depth_stencil_state: &depth_stencil,
            color_blend_state: &translucent_blending,
            dynamic_state: &dynamic_state,
            layout,
            render_pass,
            subpass: 0,
            ..Default::default()
        },
        vk::GraphicsPipelineCreateInfo {
            stage_count: stages.len() as u32,
            stages: stages.as_ptr(),
            vertex_input_state: &vertex_input,
            input_assembly_state: &input_assembly,
            viewport_state: &viewport_state,
            rasterization_state: &rasterizer,
            multisample_state: &multisampling,
            depth_stencil_state: &depth_stencil,
            color_blend_state: &overlay_blending,
            dynamic_state: &dynamic_state,
            layout,
            render_pass,
            subpass: 0,
            ..Default::default()
        },
    ];

    let mut pipelines = [vk::Pipeline::null(), vk::Pipeline::null()];
    device
        .create_graphics_pipelines(vk::PipelineCache::null(), &infos, None, &mut pipelines)
        .expect("failed to create sky pipelines");

    device.destroy_shader_module(vert_module, None);
    device.destroy_shader_module(frag_module, None);

    (pipelines[0], pipelines[1])
}
