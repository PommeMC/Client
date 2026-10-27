use std::path::Path;

use crate::assets::AssetIndex;
use crate::renderer::camera::{Camera, CameraUniform};
use crate::renderer::chunk::block_lighter::LightLevels;
use crate::renderer::chunk::mesher::BiomeClimate;
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::pipelines::sky::SkyState;
use crate::renderer::{rhi, shader, util};

/// Block radius of the rain/snow grid sampled around the camera (vanilla
/// `weatherRadius`). Also used for the per-column distance alpha falloff.
pub const WEATHER_RADIUS: i32 = 10;

const COLUMNS_PER_SIDE: usize = (2 * WEATHER_RADIUS + 1) as usize;
const MAX_WEATHER_VERTS: usize = COLUMNS_PER_SIDE * COLUMNS_PER_SIDE * 6;

/// Approximate overworld sea level; used for the height-adjusted snow line.
const SEA_LEVEL: i32 = 63;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Precip {
    None,
    Rain,
    Snow,
}

/// Classifies precipitation for a biome at a given Y, mirroring vanilla
/// `Biome.getPrecipitationAt`: no precipitation when the biome has no downfall,
/// snow when the (height-adjusted) temperature is below 0.15, else rain.
pub fn precipitation_for(climate: &BiomeClimate, y: i32) -> Precip {
    if climate.downfall <= 0.0 {
        return Precip::None;
    }
    let snow_line = SEA_LEVEL + 17;
    let temperature = if y > snow_line {
        climate.temperature - (y - snow_line) as f32 * (0.05 / 40.0)
    } else {
        climate.temperature
    };
    if temperature < 0.15 {
        Precip::Snow
    } else {
        Precip::Rain
    }
}

/// One vertical precipitation column around the player. Built CPU-side each
/// frame from chunk/biome/light data and turned into a camera-facing quad here.
pub struct WeatherColumn {
    pub x: i32,
    pub z: i32,
    pub bottom_y: f32,
    pub top_y: f32,
    pub precip: Precip,
    /// Vanilla `LightCoordsUtil.getLightCoords` at the column's light sample.
    pub light: LightLevels,
}

/// Vanilla `WeatherEffectRenderer`'s snow light: each level brightened a
/// quarter of the way to 15.
fn snow_light(light: LightLevels) -> LightLevels {
    let brighten = |level: u8| (level * 3 + 15) / 4;
    LightLevels::new(brighten(light.sky()), brighten(light.block()))
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct WeatherVertex {
    position: [f32; 3],
    uv: [f32; 2],
    alpha: f32,
    light: f32,
}

pub struct WeatherPipeline {
    pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    tex_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    rain_set: rhi::backend::BindGroup,
    snow_set: rhi::backend::BindGroup,
    camera_buffers: Vec<rhi::backend::Buffer>,
    /// The shared [`Lightmap`]'s group, set 2.
    lightmap_set: rhi::backend::BindGroup,
    vertex_buffers: Vec<rhi::backend::Buffer>,
    sampler: rhi::backend::Sampler,
    rain: rhi::backend::Texture,
    snow: rhi::backend::Texture,
}

impl WeatherPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
        lightmap: &Lightmap,
    ) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let tex_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, tex_layout, lightmap.layout],
            None,
        );

        let pipeline = create_pipeline(device, targets, pipeline_layout);

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            std::mem::size_of::<CameraUniform>() as u64,
            "weather_camera",
        );

        let vertex_buffers = util::create_per_frame_host_buffers(
            device,
            (MAX_WEATHER_VERTS * std::mem::size_of::<WeatherVertex>()) as u64,
            rhi::BufferUsages::VERTEX,
            "weather_vertices",
        );

        // Nearest sampling with REPEAT wrapping so the rain/snow strip tiles
        // vertically as the UV scrolls.
        let sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST_REPEAT);

        let rain = util::load_asset_texture(
            device,
            jar_assets_dir,
            asset_index,
            "minecraft/textures/environment/rain.png",
        );
        let rain_set = util::create_texture_group(device, tex_layout, rain.handle(), sampler);

        let snow = util::load_asset_texture(
            device,
            jar_assets_dir,
            asset_index,
            "minecraft/textures/environment/snow.png",
        );
        let snow_set = util::create_texture_group(device, tex_layout, snow.handle(), sampler);

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            tex_layout,
            camera_sets,
            rain_set,
            snow_set,
            camera_buffers,
            lightmap_set: lightmap.group,
            vertex_buffers,
            sampler,
            rain,
            snow,
        }
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
        sky: &SkyState,
        columns: &[WeatherColumn],
    ) {
        let intensity = sky.rain();
        if columns.is_empty() || intensity <= 0.0 {
            return;
        }

        // Vertices are anchor-relative, subtracted in f64 (see Camera::anchor).
        let anchor = camera.anchor();
        let cam = (*camera.position - anchor).as_vec3() + camera.third_person_offset();
        let radius_sq = (WEATHER_RADIUS as f32) * (WEATHER_RADIUS as f32);

        let mut rain_verts: Vec<WeatherVertex> = Vec::new();
        let mut snow_verts: Vec<WeatherVertex> = Vec::new();

        for col in columns {
            let (max_alpha, u_off, v_off, is_snow) = match col.precip {
                Precip::Rain => {
                    let (u, v) = rain_uv(sky.game_time, sky.partial_tick, col.x, col.z);
                    (1.0, u, v, false)
                }
                Precip::Snow => {
                    let (u, v) = snow_uv(sky.game_time, sky.partial_tick, col.x, col.z);
                    (0.8, u, v, true)
                }
                Precip::None => continue,
            };

            let wx = (col.x as f64 + 0.5 - anchor.x) as f32;
            let wz = (col.z as f64 + 0.5 - anchor.z) as f32;
            let dx = wx - cam.x;
            let dz = wz - cam.z;
            let dist_sq = dx * dx + dz * dz;
            let dist = dist_sq.sqrt();
            // Orient the quad perpendicular to the camera direction.
            let (hx, hz) = if dist < 1e-4 {
                (0.0, 0.0)
            } else {
                (-dz / dist * 0.5, dx / dist * 0.5)
            };

            // Distance falloff (vanilla lerp(min(distSq/radiusSq,1), maxAlpha, 0.5))
            // folded with rain intensity.
            let falloff = max_alpha + (0.5 - max_alpha) * (dist_sq / radius_sq).min(1.0);
            let alpha = falloff * intensity;
            let light = if is_snow {
                snow_light(col.light)
            } else {
                col.light
            }
            .lane();

            let x0 = wx - hx;
            let x1 = wx + hx;
            let z0 = wz - hz;
            let z1 = wz + hz;
            let u0 = u_off;
            let u1 = u_off + 1.0;
            // UVs keep the absolute Y so the texture phase matches vanilla.
            let v_top = col.bottom_y * 0.25 + v_off;
            let v_bot = col.top_y * 0.25 + v_off;
            let y_top = (col.top_y as f64 - anchor.y) as f32;
            let y_bot = (col.bottom_y as f64 - anchor.y) as f32;

            let corners = [
                ([x0, y_top, z0], [u0, v_top]),
                ([x1, y_top, z1], [u1, v_top]),
                ([x1, y_bot, z1], [u1, v_bot]),
                ([x0, y_bot, z0], [u0, v_bot]),
            ];

            let target = if is_snow {
                &mut snow_verts
            } else {
                &mut rain_verts
            };
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                target.push(WeatherVertex {
                    position: corners[i].0,
                    uv: corners[i].1,
                    alpha,
                    light,
                });
            }
        }

        let rain_count = rain_verts.len() as u32;
        let snow_count = snow_verts.len() as u32;
        if rain_count == 0 && snow_count == 0 {
            return;
        }

        rain_verts.append(&mut snow_verts);
        if rain_verts.len() > MAX_WEATHER_VERTS {
            rain_verts.truncate(MAX_WEATHER_VERTS);
        }
        let bytes = bytemuck::cast_slice::<WeatherVertex, u8>(&rain_verts);
        self.vertex_buffers[frame].write(bytes);

        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffers[frame].handle());
        pass.set_bind_group(0, self.camera_sets[frame]);
        pass.set_bind_group(2, self.lightmap_set);

        if rain_count > 0 {
            pass.set_bind_group(1, self.rain_set);
            pass.draw(rain_count, 1, 0, 0);
        }
        if snow_count > 0 {
            pass.set_bind_group(1, self.snow_set);
            pass.draw(snow_count, 1, rain_count, 0);
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
        std::mem::take(&mut self.rain).destroy(device);
        std::mem::take(&mut self.snow).destroy(device);

        self.pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.rain_set.destroy(device);
        self.snow_set.destroy(device);
        self.camera_layout.destroy(device);
        self.tex_layout.destroy(device);
    }
}

/// Java LCG random with `nextGaussian`, used to reproduce vanilla's per-column
/// rain speed / snow drift exactly.
struct JavaRandom {
    seed: u64,
    next_gaussian: Option<f64>,
}

impl JavaRandom {
    fn new(seed: i64) -> Self {
        Self {
            seed: (seed as u64 ^ 0x5DEECE66D) & 0xFFFF_FFFF_FFFF,
            next_gaussian: None,
        }
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(0x5DEECE66D).wrapping_add(0xB)) & 0xFFFF_FFFF_FFFF;
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

    fn next_gaussian(&mut self) -> f64 {
        if let Some(g) = self.next_gaussian.take() {
            return g;
        }
        loop {
            let v1 = 2.0 * self.next_double() - 1.0;
            let v2 = 2.0 * self.next_double() - 1.0;
            let s = v1 * v1 + v2 * v2;
            if s < 1.0 && s != 0.0 {
                let mult = (-2.0 * s.ln() / s).sqrt();
                self.next_gaussian = Some(v2 * mult);
                return v1 * mult;
            }
        }
    }
}

/// Per-column hash halves shared by the rain seed (`a ^ b`) and the rain tick
/// offset (`a + b`), matching vanilla's column index math.
fn column_hash_parts(x: i32, z: i32) -> (i32, i32) {
    let a = x
        .wrapping_mul(x)
        .wrapping_mul(3121)
        .wrapping_add(x.wrapping_mul(45238971));
    let b = z
        .wrapping_mul(z)
        .wrapping_mul(418711)
        .wrapping_add(z.wrapping_mul(13761));
    (a, b)
}

fn column_seed(x: i32, z: i32) -> i64 {
    let (a, b) = column_hash_parts(x, z);
    (a ^ b) as i64
}

fn column_tick_offset(x: i32, z: i32) -> i32 {
    let (a, b) = column_hash_parts(x, z);
    a.wrapping_add(b) & 0xFF
}

/// Vanilla `createRainColumnInstance`: V scrolls downward at `3 + rand` speed.
fn rain_uv(game_time: u64, partial: f32, x: i32, z: i32) -> (f32, f32) {
    let wrapped = (game_time & 0x1FFFF) as f32;
    let tick_offset = column_tick_offset(x, z) as f32;
    let mut rng = JavaRandom::new(column_seed(x, z));
    let speed = 3.0 + rng.next_float();
    let texture_offset = -((wrapped + tick_offset) + partial) / 32.0 * speed;
    (0.0, texture_offset % 32.0)
}

/// Vanilla `createSnowColumnInstance`: gentle gaussian drift plus a slow
/// scroll.
fn snow_uv(game_time: u64, partial: f32, x: i32, z: i32) -> (f32, f32) {
    let mut rng = JavaRandom::new(column_seed(x, z));
    let wrapped = (game_time & 0x1FFFF) as f32;
    let time = wrapped + partial;
    let u = rng.next_double() as f32 + time * 0.01 * rng.next_gaussian() as f32;
    let v_drift = rng.next_double() as f32 + time * rng.next_gaussian() as f32 * 0.001;
    let v_base = -(((game_time & 0x1FF) as f32) + partial) / 512.0;
    (u, v_base + v_drift)
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
            // Depth-test against the world (rain is occluded by terrain) but do
            // NOT write depth, so overlapping translucent columns blend and the
            // later hand/HUD passes are unaffected.
            depth: Some(rhi::DepthState {
                compare: rhi::CompareFunction::Less,
                ..rhi::DepthState::READ_ONLY
            }),
            blend: Some(rhi::BlendState::TRANSLUCENT),
            ..rhi::RenderPipelineDesc::new(
                "weather",
                shader::include_shader!("weather.vert"),
                shader::include_shader!("weather.frag"),
                &[rhi::vertex_buffer!(
                    WeatherVertex,
                    [Float32x3, Float32x2, Float32, Float32]
                )],
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snow_brightens_each_level_a_quarter_toward_15() {
        assert_eq!(snow_light(LightLevels::new(0, 0)), LightLevels::new(3, 3));
        assert_eq!(snow_light(LightLevels::new(15, 4)), LightLevels::new(15, 6));
        assert_eq!(snow_light(LightLevels::new(7, 14)), LightLevels::new(9, 14));
    }
}
