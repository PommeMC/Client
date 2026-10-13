use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};

use azalea_registry::builtin::EntityKind;
use glam::{DVec3, Vec3};
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::camera::CameraUniform;
use crate::renderer::pipelines::entity_renderer::EntityRenderInfo;
use crate::renderer::pipelines::item_entity::ItemRenderInfo;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};
use crate::world::block::block_shape;
use crate::world::chunk::ChunkStore;

/// Per-vertex data for shadow quads.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShadowVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
    pub uv: [f32; 2],
}

/// Buffer capacity in vertices (each shadow part is a quad = 6 vertices: 2
/// triangles). 16384 vertices accommodates ~2730 shadow quads per frame.
const MAX_SHADOW_VERTS: usize = 16384;

#[allow(dead_code)]
pub struct EntityShadowPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    texture_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    texture_set: vk::DescriptorSet,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Allocation>,
    vertex_buffers: Vec<vk::Buffer>,
    vertex_allocations: Vec<Allocation>,
    texture_image: vk::Image,
    texture_view: vk::ImageView,
    texture_sampler: vk::Sampler,
    texture_allocation: Allocation,
}

impl EntityShadowPipeline {
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
        let texture_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::CombinedImageSampler,
            vk::ShaderStageFlags::Fragment,
        );

        let layouts = [camera_layout, texture_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            push_constant_range_count: 0,
            push_constant_ranges: std::ptr::null(),
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create entity shadow pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: (MAX_FRAMES_IN_FLIGHT + 1) as u32,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create entity shadow descriptor pool");

        let cam_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| camera_layout).collect();
        let cam_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: cam_layouts.len() as u32,
            set_layouts: cam_layouts.as_ptr(),
            ..Default::default()
        };
        let mut camera_sets = vec![vk::DescriptorSet::null(); cam_layouts.len()];
        device
            .allocate_descriptor_sets(&cam_alloc_info, &mut camera_sets)
            .expect("failed to allocate entity shadow camera sets");

        let tex_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &texture_layout,
            ..Default::default()
        };
        let mut texture_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&tex_alloc_info, slice::from_mut(&mut texture_set))
            .expect("failed to allocate entity shadow texture set");

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                size_of::<CameraUniform>() as u64,
                "entity_shadow_camera",
            );
            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: size_of::<CameraUniform>() as u64,
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
            camera_allocations.push(alloc);
        }

        let (texture_image, texture_view, texture_allocation) = load_shadow_texture(
            device,
            queue,
            command_pool,
            allocator,
            jar_assets_dir,
            asset_index,
        );

        let texture_sampler = unsafe { util::create_linear_sampler(device) };

        let image_info = vk::DescriptorImageInfo {
            sampler: texture_sampler,
            image_view: texture_view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let tex_write = vk::WriteDescriptorSet {
            dst_set: texture_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            buffer_info: std::ptr::null(),
            image_info: &image_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[tex_write], &[]);

        let placeholder = vec![
            ShadowVertex {
                position: [0.0; 3],
                color: [0.0; 4],
                uv: [0.0; 2],
            };
            MAX_SHADOW_VERTS
        ];
        let bytes = bytemuck::cast_slice::<ShadowVertex, u8>(&placeholder);
        let mut vertex_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut vertex_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            let (buf, alloc) = util::create_mapped_buffer(
                device,
                allocator,
                bytes,
                vk::BufferUsageFlags::VertexBuffer,
                "entity_shadow_vertices",
            );
            vertex_buffers.push(buf);
            vertex_allocations.push(alloc);
        }

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            texture_layout,
            descriptor_pool,
            camera_sets,
            texture_set,
            camera_buffers,
            camera_allocations,
            vertex_buffers,
            vertex_allocations,
            texture_image,
            texture_view,
            texture_sampler,
            texture_allocation,
        }
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);
    }

    /// Renders shadows for all given entities and item entities onto blocks
    /// below.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        chunks: &ChunkStore,
        entities: &[EntityRenderInfo],
        item_entities: &[ItemRenderInfo],
        anchor: DVec3,
        camera_pos: DVec3,
        cull_distance: f32,
    ) {
        let mut verts: Vec<ShadowVertex> = Vec::new();

        // 1. Living entities / Players
        for entity in entities {
            if entity.is_sleeping {
                continue;
            }
            let (radius, strength) = entity_shadow_params(entity.entity_kind, entity.is_baby);
            if radius <= 0.0 || strength <= 0.0 {
                continue;
            }

            let ent_pos = DVec3::new(entity.position.x, entity.position.y, entity.position.z);

            let dist_to_camera_sq = ent_pos.distance_squared(camera_pos);
            if dist_to_camera_sq > (cull_distance as f64).powi(2) {
                continue;
            }

            // Vanilla distance opacity falloff: (1.0 - distSq / 256.0) * strength
            let dist_factor = 1.0 - dist_to_camera_sq / 256.0;
            if dist_factor <= 0.0 {
                continue;
            }
            let opacity = (dist_factor as f32) * strength;
            if opacity <= 0.0 {
                continue;
            }

            extract_shadow_quads(&mut verts, chunks, ent_pos, anchor, radius, opacity);

            if verts.len() >= MAX_SHADOW_VERTS {
                break;
            }
        }

        // 2. Item entities (radius 0.15, strength 0.75 per issue #55 and vanilla)
        for item in item_entities {
            let col3 = item.model_matrix.col(3);
            let ent_pos = anchor + DVec3::new(col3.x as f64, col3.y as f64, col3.z as f64);

            let dist_to_camera_sq = ent_pos.distance_squared(camera_pos);
            if dist_to_camera_sq > (cull_distance as f64).powi(2) {
                continue;
            }

            let dist_factor = 1.0 - dist_to_camera_sq / 256.0;
            if dist_factor <= 0.0 {
                continue;
            }
            let opacity = (dist_factor as f32) * 0.75;
            if opacity <= 0.0 {
                continue;
            }

            extract_shadow_quads(&mut verts, chunks, ent_pos, anchor, 0.15, opacity);

            if verts.len() >= MAX_SHADOW_VERTS {
                break;
            }
        }

        if verts.is_empty() {
            return;
        }

        let vert_count = verts.len().min(MAX_SHADOW_VERTS);
        let bytes = bytemuck::cast_slice::<ShadowVertex, u8>(&verts[..vert_count]);
        self.vertex_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.camera_sets[frame], self.texture_set],
            &[],
        );
        cmd.bind_vertex_buffers(0, &[self.vertex_buffers[frame]], &[0]);
        cmd.draw(vert_count as u32, 1, 0, 0);
    }
}

impl Drop for EntityShadowPipeline {
    fn drop(&mut self) {
        // Vulkan resources cleaned up by Renderer teardown / device wait idle
    }
}

/// Vanilla `EntityRenderer.getShadowRadius` and `shadowStrength` per mob kind.
pub fn entity_shadow_params(kind: EntityKind, is_baby: bool) -> (f32, f32) {
    let strength = 1.0;
    let radius = match kind {
        EntityKind::Item => return (0.15, 0.75),
        EntityKind::ExperienceOrb => return (0.15, 0.5),
        EntityKind::ArmorStand => return (0.0, 0.0),
        EntityKind::Player => 0.5,
        EntityKind::Pig => 0.7,
        EntityKind::Cow => 0.7,
        EntityKind::Chicken => 0.3,
        EntityKind::Sheep => 0.7,
        EntityKind::Zombie
        | EntityKind::Husk
        | EntityKind::Drowned
        | EntityKind::ZombieVillager
        | EntityKind::Villager
        | EntityKind::Witch => 0.5,
        EntityKind::Skeleton | EntityKind::Stray | EntityKind::Bogged => 0.5,
        EntityKind::Creeper => 0.5,
        EntityKind::Spider => 0.8,
        EntityKind::CaveSpider => 0.5,
        EntityKind::Enderman => 0.5,
        EntityKind::Endermite => 0.3,
        EntityKind::Silverfish => 0.3,
        EntityKind::Slime => 0.25,
        EntityKind::Wolf => 0.5,
        EntityKind::Cat | EntityKind::Ocelot => 0.4,
        EntityKind::Rabbit => 0.3,
        EntityKind::Horse
        | EntityKind::Donkey
        | EntityKind::Mule
        | EntityKind::SkeletonHorse
        | EntityKind::ZombieHorse => 0.75,
        EntityKind::Squid | EntityKind::GlowSquid => 0.7,
        EntityKind::Bat => 0.25,
        EntityKind::Cod => 0.3,
        EntityKind::Salmon => 0.4,
        EntityKind::TropicalFish => 0.15,
        EntityKind::Pufferfish => 0.2,
        EntityKind::IronGolem => 0.7,
        EntityKind::SnowGolem => 0.5,
        EntityKind::Ghast => 1.5,
        EntityKind::Warden => 0.9,
        EntityKind::PolarBear => 0.9,
        EntityKind::Panda => 0.9,
        EntityKind::Turtle => 0.7,
        EntityKind::Dolphin => 0.7,
        EntityKind::Fox => 0.4,
        EntityKind::Bee => 0.4,
        EntityKind::Llama => 0.7,
        EntityKind::Camel => 0.7,
        EntityKind::Ravager => 1.1,
        EntityKind::Sniffer => 1.1,
        EntityKind::Allay => 0.4,
        EntityKind::Vex => 0.3,
        EntityKind::Frog => 0.3,
        EntityKind::Tadpole => 0.14,
        EntityKind::Blaze => 0.5,
        EntityKind::Breeze => 0.5,
        EntityKind::Wither => 1.0,
        _ => 0.5,
    };
    let scale = if is_baby { 0.5 } else { 1.0 };
    (radius * scale, strength)
}

/// Vanilla `extractShadowPiece` & `ShadowFeatureRenderer.prepare` geometry
/// builder. Projects shadow quads onto the solid block top surfaces below the
/// entity position.
fn extract_shadow_quads(
    verts: &mut Vec<ShadowVertex>,
    chunks: &ChunkStore,
    ent_pos: DVec3,
    anchor: DVec3,
    radius: f32,
    opacity: f32,
) {
    let min_bx = (ent_pos.x - radius as f64).floor() as i32;
    let max_bx = (ent_pos.x + radius as f64).floor() as i32;
    let min_bz = (ent_pos.z - radius as f64).floor() as i32;
    let max_bz = (ent_pos.z + radius as f64).floor() as i32;

    // y_span = ((opacity / 0.5) - 1.0).min(radius)
    let y_span = ((opacity / 0.5) - 1.0).min(radius);
    let min_by = (ent_pos.y - y_span as f64).floor() as i32;
    let max_by = ent_pos.y.floor() as i32;

    for bz in min_bz..=max_bz {
        for bx in min_bx..=max_bx {
            for by in min_by..=max_by {
                // Check block below (by - 1)
                let block_below = chunks.get_block_state(bx, by - 1, bz);
                if block_below.is_air() {
                    continue;
                }

                // Check light level at current (air/passable) block (bx, by, bz)
                let sky_light = chunks.get_sky_light(bx, by, bz);
                let block_light = chunks.get_block_light(bx, by, bz);
                let raw_light = sky_light.max(block_light);
                if raw_light <= 3 {
                    continue;
                }

                // Collision shape check
                let Some(boxes) = block_shape(block_below) else {
                    continue;
                };

                let bounds = match calculate_box_bounds(boxes) {
                    Some(b) => b,
                    None => continue,
                };

                // Vertical distance from entity Y to the top of the block bounding box:
                // d = ent_pos.y - ((by - 1) as f64 + bounds.max_y)
                let block_top_y = (by - 1) as f64 + bounds[4];
                let d = (ent_pos.y - block_top_y) as f32;
                if d < 0.0 {
                    continue;
                }

                // Opacity falloff with height: alpha = (opacity - d * 0.5) * 0.5 *
                // light_brightness
                let v = raw_light as f32 / 15.0;
                let curved_light = v / (4.0 - 3.0 * v);
                let alpha = ((opacity - d * 0.5) * 0.5 * curved_light).clamp(0.0, 1.0);
                if alpha <= 0.0 {
                    continue;
                }

                // Relative coordinates to entity center:
                // rel_x = (bx as f64 - ent_pos.x) as f32
                // rel_y = ((by - 1) as f64 - ent_pos.y) as f32
                // rel_z = (bz as f64 - ent_pos.z) as f32
                let rel_x = (bx as f64 - ent_pos.x) as f32;
                let rel_y = ((by - 1) as f64 - ent_pos.y) as f32;
                let rel_z = (bz as f64 - ent_pos.z) as f32;

                let p_x0 = rel_x + bounds[0] as f32;
                let p_x1 = rel_x + bounds[3] as f32;
                let p_y = rel_y + bounds[4] as f32 + 0.005; // 0.005 lift to prevent z-fighting
                let p_z0 = rel_z + bounds[2] as f32;
                let p_z1 = rel_z + bounds[5] as f32;

                // Vanilla UV formula from ShadowFeatureRenderer:
                // u = -p_x / (2.0 * radius) + 0.5
                // v = -p_z / (2.0 * radius) + 0.5
                let u0 = -p_x0 / (2.0 * radius) + 0.5;
                let u1 = -p_x1 / (2.0 * radius) + 0.5;
                let v0 = -p_z0 / (2.0 * radius) + 0.5;
                let v1 = -p_z1 / (2.0 * radius) + 0.5;

                // World position relative to camera anchor:
                // world = (ent_pos - anchor) + [p_x, p_y, p_z]
                let base_anchor_rel = (ent_pos - anchor).as_vec3();
                let v_pos = |x: f32, z: f32| -> [f32; 3] {
                    (base_anchor_rel + Vec3::new(x, p_y, z)).to_array()
                };

                let color = [1.0, 1.0, 1.0, alpha];

                // Quad vertices (2 triangles: 0, 1, 2 and 0, 2, 3)
                // Corners:
                // C0: (p_x0, p_z0) -> UV: (u0, v0)
                // C1: (p_x0, p_z1) -> UV: (u0, v1)
                // C2: (p_x1, p_z1) -> UV: (u1, v1)
                // C3: (p_x1, p_z0) -> UV: (u1, v0)
                let c0 = ShadowVertex {
                    position: v_pos(p_x0, p_z0),
                    color,
                    uv: [u0, v0],
                };
                let c1 = ShadowVertex {
                    position: v_pos(p_x0, p_z1),
                    color,
                    uv: [u0, v1],
                };
                let c2 = ShadowVertex {
                    position: v_pos(p_x1, p_z1),
                    color,
                    uv: [u1, v1],
                };
                let c3 = ShadowVertex {
                    position: v_pos(p_x1, p_z0),
                    color,
                    uv: [u1, v0],
                };

                verts.push(c0);
                verts.push(c1);
                verts.push(c2);
                verts.push(c0);
                verts.push(c2);
                verts.push(c3);

                if verts.len() >= MAX_SHADOW_VERTS {
                    return;
                }
            }
        }
    }
}

fn calculate_box_bounds(boxes: &[crate::physics::block_shape::LocalBox]) -> Option<[f64; 6]> {
    let &first = boxes.first()?;
    let mut b = first;
    for bx in &boxes[1..] {
        for i in 0..3 {
            b[i] = b[i].min(bx[i]);
            b[i + 3] = b[i + 3].max(bx[i + 3]);
        }
    }
    Some(b)
}

fn load_shadow_texture(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) -> (vk::Image, vk::ImageView, Allocation) {
    let key = "minecraft/textures/misc/shadow.png";
    let path = resolve_asset_path(jar_assets_dir, asset_index, key);
    let (pixels, w, h) = if let Some((p, width, height)) = util::load_png(&path) {
        (p, width, height)
    } else {
        tracing::warn!(
            "Failed to load shadow texture from {}, using fallback",
            path.display()
        );
        // 64x64 fallback circular gradient
        let mut p = Vec::with_capacity(64 * 64 * 4);
        for y in 0..64 {
            for x in 0..64 {
                let dx = (x as f32 - 31.5) / 32.0;
                let dy = (y as f32 - 31.5) / 32.0;
                let dist = (dx * dx + dy * dy).sqrt();
                let a = ((1.0 - dist).clamp(0.0, 1.0) * 255.0) as u8;
                p.extend_from_slice(&[0, 0, 0, a]);
            }
        }
        (p, 64, 64)
    };

    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, w, h, "entity_shadow_texture");
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, &pixels, "entity_shadow_staging");

    util::upload_image(device, queue, command_pool, staging_buf, image, w, h);

    device.destroy_buffer(staging_buf, None);
    let _ = allocator.lock().unwrap().free(staging_alloc);

    (image, view, allocation)
}

fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("entity_shadow.vert.spv");
    let frag_spv = shader::include_spirv!("entity_shadow.frag.spv");

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

    let binding_desc = vk::VertexInputBindingDescription {
        binding: 0,
        stride: size_of::<ShadowVertex>() as u32,
        input_rate: vk::VertexInputRate::Vertex,
    };

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
            format: vk::Format::R32G32B32A32Sfloat,
            offset: 12,
        },
        vk::VertexInputAttributeDescription {
            location: 2,
            binding: 0,
            format: vk::Format::R32G32Sfloat,
            offset: 28,
        },
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: 1,
        vertex_binding_descriptions: &binding_desc,
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
        depth_test_enable: vk::TRUE,
        depth_write_enable: vk::FALSE,
        depth_compare_op: vk::CompareOp::LessOrEqual,
        ..Default::default()
    };

    let blend_attachment = vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::SrcAlpha,
        dst_color_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: vk::BlendFactor::One,
        dst_alpha_blend_factor: vk::BlendFactor::Zero,
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

    let pipeline_info = [vk::GraphicsPipelineCreateInfo {
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

    let mut pipelines = vec![vk::Pipeline::null()];
    device
        .create_graphics_pipelines(
            vk::PipelineCache::null(),
            &pipeline_info,
            None,
            &mut pipelines,
        )
        .expect("failed to create entity shadow graphics pipeline");

    device.destroy_shader_module(vert_module, None);
    device.destroy_shader_module(frag_module, None);

    pipelines[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_shadow_params_item() {
        let (radius, strength) = entity_shadow_params(EntityKind::Item, false);
        assert!((radius - 0.15).abs() < 1e-4);
        assert!((strength - 0.75).abs() < 1e-4);
    }

    #[test]
    fn test_entity_shadow_params_player() {
        let (radius, strength) = entity_shadow_params(EntityKind::Player, false);
        assert!((radius - 0.5).abs() < 1e-4);
        assert!((strength - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_entity_shadow_params_baby() {
        let (radius_adult, _) = entity_shadow_params(EntityKind::Cow, false);
        let (radius_baby, _) = entity_shadow_params(EntityKind::Cow, true);
        assert!((radius_adult - 0.7).abs() < 1e-4);
        assert!((radius_baby - 0.35).abs() < 1e-4);
    }

    #[test]
    fn test_entity_shadow_params_armor_stand() {
        let (radius, strength) = entity_shadow_params(EntityKind::ArmorStand, false);
        assert_eq!(radius, 0.0);
        assert_eq!(strength, 0.0);
    }
}
