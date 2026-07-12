use std::path::Path;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::camera::{Camera, CameraUniform};
use crate::renderer::chunk::mesher::BiomeClimate;
use crate::renderer::pipelines::sky::SkyState;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};

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
    /// Skylight/blocklight brightness factor (0..1) at the column.
    pub light: f32,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct WeatherVertex {
    position: [f32; 3],
    uv: [f32; 2],
    brightness: f32,
}

pub struct WeatherPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    tex_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    rain_set: vk::DescriptorSet,
    snow_set: vk::DescriptorSet,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Option<Allocation>>,
    vertex_buffers: Vec<vk::Buffer>,
    vertex_allocations: Vec<Option<Allocation>>,
    sampler: vk::Sampler,
    rain_image: vk::Image,
    rain_view: vk::ImageView,
    rain_allocation: Option<Allocation>,
    snow_image: vk::Image,
    snow_view: vk::ImageView,
    snow_allocation: Option<Allocation>,
}

impl WeatherPipeline {
    pub fn new(
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let camera_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Vertex,
        );
        let tex_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::CombinedImageSampler,
            vk::ShaderStageFlags::Fragment,
        );

        let layouts = [camera_layout, tex_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create weather pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 2,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: (MAX_FRAMES_IN_FLIGHT + 2) as u32,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create weather descriptor pool");

        let camera_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| camera_layout).collect();
        let camera_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: camera_layouts.len() as u32,
            set_layouts: camera_layouts.as_ptr(),
            ..Default::default()
        };
        let mut camera_sets = vec![vk::DescriptorSet::null(); camera_layouts.len()];
        device
            .allocate_descriptor_sets(&camera_alloc_info, &mut camera_sets)
            .expect("failed to allocate weather camera sets");

        let tex_layouts = [tex_layout, tex_layout];
        let tex_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: tex_layouts.len() as u32,
            set_layouts: tex_layouts.as_ptr(),
            ..Default::default()
        };
        let mut tex_sets = vec![vk::DescriptorSet::null(); 2];
        device
            .allocate_descriptor_sets(&tex_alloc_info, &mut tex_sets)
            .expect("failed to allocate weather texture sets");
        let rain_set = tex_sets[0];
        let snow_set = tex_sets[1];

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations: Vec<Option<Allocation>> =
            Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<CameraUniform>() as u64,
                "weather_camera",
            );
            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<CameraUniform>() as u64,
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
            camera_buffers.push(buf);
            camera_allocations.push(Some(alloc));
        }

        let mut vertex_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut vertex_allocations: Vec<Option<Allocation>> =
            Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let vertex_bytes = (MAX_WEATHER_VERTS * std::mem::size_of::<WeatherVertex>()) as u64;
        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            let (buf, alloc) = util::create_host_buffer(
                device,
                allocator,
                vertex_bytes,
                vk::BufferUsageFlags::VertexBuffer,
                "weather_vertices",
            );
            vertex_buffers.push(buf);
            vertex_allocations.push(Some(alloc));
        }

        // Nearest sampling with REPEAT wrapping so the rain/snow strip tiles
        // vertically as the UV scrolls.
        let sampler = create_repeat_sampler(device);

        let (rain_image, rain_view, rain_allocation) = load_weather_texture(
            device,
            queue,
            command_pool,
            allocator,
            jar_assets_dir,
            asset_index,
            "minecraft/textures/environment/rain.png",
        );
        bind_texture_set(device, rain_set, rain_view, sampler);

        let (snow_image, snow_view, snow_allocation) = load_weather_texture(
            device,
            queue,
            command_pool,
            allocator,
            jar_assets_dir,
            asset_index,
            "minecraft/textures/environment/snow.png",
        );
        bind_texture_set(device, snow_set, snow_view, sampler);

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            tex_layout,
            descriptor_pool,
            camera_sets,
            rain_set,
            snow_set,
            camera_buffers,
            camera_allocations,
            vertex_buffers,
            vertex_allocations,
            sampler,
            rain_image,
            rain_view,
            rain_allocation: Some(rain_allocation),
            snow_image,
            snow_view,
            snow_allocation: Some(snow_allocation),
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        if let Some(alloc) = self.camera_allocations[frame].as_mut() {
            alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
        }
    }

    pub fn update_and_draw(
        &mut self,
        cmd: vk::CommandBuffer,
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
            // folded with rain intensity and the column light.
            let falloff = max_alpha + (0.5 - max_alpha) * (dist_sq / radius_sq).min(1.0);
            let brightness = falloff * intensity * col.light;

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
                    brightness,
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
        if let Some(alloc) = self.vertex_allocations[frame].as_mut() {
            alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
        }

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_vertex_buffers(0, &[self.vertex_buffers[frame]], &[0]);

        if rain_count > 0 {
            cmd.bind_descriptor_sets(
                vk::PipelineBindPoint::Graphics,
                self.pipeline_layout,
                0,
                &[self.camera_sets[frame], self.rain_set],
                &[],
            );
            cmd.draw(rain_count, 1, 0, 0);
        }
        if snow_count > 0 {
            cmd.bind_descriptor_sets(
                vk::PipelineBindPoint::Graphics,
                self.pipeline_layout,
                0,
                &[self.camera_sets[frame], self.snow_set],
                &[],
            );
            cmd.draw(snow_count, 1, rain_count, 0);
        }
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.camera_buffers[i], None);
            if let Some(a) = self.camera_allocations[i].take() {
                alloc.free(a).ok();
            }
            device.destroy_buffer(self.vertex_buffers[i], None);
            if let Some(a) = self.vertex_allocations[i].take() {
                alloc.free(a).ok();
            }
        }

        device.destroy_sampler(self.sampler, None);
        device.destroy_image_view(self.rain_view, None);
        device.destroy_image(self.rain_image, None);
        if let Some(a) = self.rain_allocation.take() {
            alloc.free(a).ok();
        }
        device.destroy_image_view(self.snow_view, None);
        device.destroy_image(self.snow_image, None);
        if let Some(a) = self.snow_allocation.take() {
            alloc.free(a).ok();
        }
        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.camera_layout, None);
        device.destroy_descriptor_set_layout(self.tex_layout, None);
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

fn create_repeat_sampler(device: &vk::Device) -> vk::Sampler {
    let info = vk::SamplerCreateInfo {
        mag_filter: vk::Filter::Nearest,
        min_filter: vk::Filter::Nearest,
        address_mode_u: vk::SamplerAddressMode::Repeat,
        address_mode_v: vk::SamplerAddressMode::Repeat,
        address_mode_w: vk::SamplerAddressMode::Repeat,
        ..Default::default()
    };
    device
        .create_sampler(&info, None)
        .expect("failed to create weather sampler")
}

fn load_weather_texture(
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

    tracing::info!("Weather: loaded {key} ({w}x{h})");
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

fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("weather.vert.spv");
    let frag_spv = shader::include_spirv!("weather.frag.spv");
    let vert_mod = shader::create_shader_module(device, vert_spv);
    let frag_mod = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Vertex,
            module: vert_mod,
            name: c"main".as_ptr(),
            ..Default::default()
        },
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Fragment,
            module: frag_mod,
            name: c"main".as_ptr(),
            ..Default::default()
        },
    ];

    let binding_descs = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: std::mem::size_of::<WeatherVertex>() as u32,
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
        vk::VertexInputAttributeDescription {
            location: 2,
            binding: 0,
            format: vk::Format::R32Sfloat,
            offset: 20,
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
    // Depth-test against the world (rain is occluded by terrain) but do NOT
    // write depth, so overlapping translucent columns blend and the later
    // hand/HUD passes are unaffected.
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo {
        depth_test_enable: vk::TRUE,
        depth_write_enable: vk::FALSE,
        depth_compare_op: vk::CompareOp::Less,
        ..Default::default()
    };
    let blend_attachment = vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::SrcAlpha,
        dst_color_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: vk::BlendFactor::One,
        dst_alpha_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        alpha_blend_op: vk::BlendOp::Add,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    };
    let color_blending = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: 1,
        attachments: &blend_attachment,
        ..Default::default()
    };
    let dynamic_states = [vk::DynamicState::Viewport, vk::DynamicState::Scissor];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo {
        dynamic_state_count: dynamic_states.len() as u32,
        dynamic_states: dynamic_states.as_ptr(),
        ..Default::default()
    };

    let info = [vk::GraphicsPipelineCreateInfo {
        stage_count: stages.len() as u32,
        stages: stages.as_ptr(),
        vertex_input_state: &vertex_input,
        input_assembly_state: &input_assembly,
        viewport_state: &viewport_state,
        rasterization_state: &rasterizer,
        multisample_state: &multisampling,
        depth_stencil_state: &depth_stencil,
        color_blend_state: &color_blending,
        dynamic_state: &dynamic_state,
        layout,
        render_pass,
        subpass: 0,
        ..Default::default()
    }];

    let mut pipeline = [vk::Pipeline::null()];
    device
        .create_graphics_pipelines(vk::PipelineCache::null(), &info, None, &mut pipeline)
        .expect("failed to create weather pipeline");

    device.destroy_shader_module(vert_mod, None);
    device.destroy_shader_module(frag_mod, None);

    pipeline[0]
}
