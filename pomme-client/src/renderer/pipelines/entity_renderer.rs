use std::collections::HashMap;
use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};

use azalea_registry::builtin::EntityKind;
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::entity::components::Position;
use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::mesher::ChunkVertex;
use crate::renderer::entity_model::BakedEntityModel;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, entity_model, shader, util};

pub const MAX_OVERLAYS: usize = 2;

pub struct EntityRenderInfo {
    pub position: Position,
    pub head_x_rot_deg: f32,
    pub head_y_rot_deg: f32,
    pub body_y_rot_deg: f32,
    pub is_baby: bool,
    pub is_crouching: bool,
    pub walk_anim_pos: f32,
    pub walk_anim_speed: f32,
    pub entity_kind: EntityKind,
    pub variant_index: u32,
    pub overlay_tints: [Option<[f32; 4]>; MAX_OVERLAYS],
    pub head_y_offset: f32,
    pub head_x_rot_deg_override: Option<f32>,
    pub has_red_overlay: bool,
}

struct MobVariant {
    model: BakedEntityModel,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Allocation,
    texture_image: vk::Image,
    texture_view: vk::ImageView,
    texture_allocation: Allocation,
    texture_set: vk::DescriptorSet,
}

struct MobEntry {
    adult_variants: Vec<MobVariant>,
    baby_variants: Option<Vec<MobVariant>>,
    adult_overlays: Vec<MobVariant>,
    baby_overlays: Vec<MobVariant>,
    anim: AnimationType,
}

impl MobEntry {
    fn base_variant(&self, is_baby: bool, variant_index: u32) -> &MobVariant {
        let pool = if is_baby {
            self.baby_variants.as_ref().unwrap_or(&self.adult_variants)
        } else {
            &self.adult_variants
        };
        let idx = (variant_index as usize).min(pool.len().saturating_sub(1));
        &pool[idx]
    }

    fn overlays(&self, is_baby: bool) -> &[MobVariant] {
        if is_baby {
            &self.baby_overlays
        } else {
            &self.adult_overlays
        }
    }
}

pub const WHITE_TINT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Vanilla `OverlayTexture` hurt pixel (ARGB 0xB2FF0000): rgb is the overlay
/// color, `a` is how much of the base color survives the mix.
const HURT_OVERLAY: [f32; 4] = [1.0, 0.0, 0.0, 178.0 / 255.0];
const NO_OVERLAY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

pub const WOOL_COLOR_RGBA: [[f32; 4]; 16] = [
    rgb(0xF0F0F0), // 0 white
    rgb(0xEB8844), // 1 orange
    rgb(0xC354CD), // 2 magenta
    rgb(0x6689D3), // 3 light_blue
    rgb(0xDECF2A), // 4 yellow
    rgb(0x41CD34), // 5 lime
    rgb(0xD88198), // 6 pink
    rgb(0x434343), // 7 gray
    rgb(0xABABAB), // 8 light_gray
    rgb(0x287697), // 9 cyan
    rgb(0x7B2FBE), // 10 purple
    rgb(0x253192), // 11 blue
    rgb(0x51301A), // 12 brown
    rgb(0x3B511A), // 13 green
    rgb(0xB3312C), // 14 red
    rgb(0x1E1B1B), // 15 black
];

const fn rgb(hex: u32) -> [f32; 4] {
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    [r, g, b, 1.0]
}

pub fn wool_color_tint(color: u8) -> [f32; 4] {
    WOOL_COLOR_RGBA[(color & 0x0F) as usize]
}

pub fn jeb_sheep_tint(entity_id: i32, age_in_ticks: u32) -> [f32; 4] {
    let base = (age_in_ticks / 25).wrapping_add(entity_id as u32);
    let c1 = (base % 16) as usize;
    let c2 = ((base + 1) % 16) as usize;
    let t = (age_in_ticks % 25) as f32 / 25.0;
    let a = WOOL_COLOR_RGBA[c1];
    let b = WOOL_COLOR_RGBA[c2];
    [
        a[0] * (1.0 - t) + b[0] * t,
        a[1] * (1.0 - t) + b[1] * t,
        a[2] * (1.0 - t) + b[2] * t,
        1.0,
    ]
}

pub struct EntityRenderer {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    texture_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Allocation>,
    texture_sampler: vk::Sampler,
    mobs: HashMap<EntityKind, MobEntry>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimationType {
    Quadruped,
    Humanoid,
}

struct VariantDef {
    model: BakedEntityModel,
    /// Outer slice: one entry per texture variant (variant_index). Inner slice:
    /// fallback chain of asset keys.
    tex_variants: &'static [&'static [&'static str]],
    tex_size: u32,
}

struct MobDef {
    kind: EntityKind,
    anim: AnimationType,
    adult: VariantDef,
    baby: Option<VariantDef>,
    adult_overlays: Vec<VariantDef>,
    baby_overlays: Vec<VariantDef>,
}

fn mob_definitions() -> Vec<MobDef> {
    const PIG_ADULT_TEX: &[&[&str]] = &[&[
        "minecraft/textures/entity/pig/pig_temperate.png",
        "minecraft/textures/entity/pig/temperate_pig.png",
    ]];
    const PIG_BABY_TEX: &[&[&str]] = &[&["minecraft/textures/entity/pig/pig_temperate_baby.png"]];
    const COW_ADULT_TEX: &[&[&str]] = &[
        &[
            "minecraft/textures/entity/cow/cow_temperate.png",
            "minecraft/textures/entity/cow/cow.png",
        ],
        &["minecraft/textures/entity/cow/cow_cold.png"],
        &["minecraft/textures/entity/cow/cow_warm.png"],
    ];
    const COW_BABY_TEX: &[&[&str]] = &[
        &["minecraft/textures/entity/cow/cow_temperate_baby.png"],
        &["minecraft/textures/entity/cow/cow_cold_baby.png"],
        &["minecraft/textures/entity/cow/cow_warm_baby.png"],
    ];
    const SHEEP_ADULT_TEX: &[&[&str]] = &[&["minecraft/textures/entity/sheep/sheep.png"]];
    const SHEEP_BABY_TEX: &[&[&str]] = &[&["minecraft/textures/entity/sheep/sheep_baby.png"]];
    const SHEEP_WOOL_UNDERCOAT_TEX: &[&[&str]] =
        &[&["minecraft/textures/entity/sheep/sheep_wool_undercoat.png"]];
    const SHEEP_WOOL_TEX: &[&[&str]] = &[&["minecraft/textures/entity/sheep/sheep_wool.png"]];
    const SHEEP_BABY_WOOL_TEX: &[&[&str]] =
        &[&["minecraft/textures/entity/sheep/sheep_wool_baby.png"]];
    const PLAYER_TEX: &[&[&str]] = &[&["minecraft/textures/entity/player/wide/steve.png"]];

    vec![
        MobDef {
            kind: EntityKind::Pig,
            anim: AnimationType::Quadruped,
            adult: VariantDef {
                model: entity_model::bake_pig_model(),
                tex_variants: PIG_ADULT_TEX,
                tex_size: 64,
            },
            baby: Some(VariantDef {
                model: entity_model::bake_baby_pig_model(),
                tex_variants: PIG_BABY_TEX,
                tex_size: 32,
            }),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Cow,
            anim: AnimationType::Quadruped,
            adult: VariantDef {
                model: entity_model::bake_cow_model(),
                tex_variants: COW_ADULT_TEX,
                tex_size: 64,
            },
            baby: Some(VariantDef {
                model: entity_model::bake_baby_cow_model(),
                tex_variants: COW_BABY_TEX,
                tex_size: 64,
            }),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Sheep,
            anim: AnimationType::Quadruped,
            adult: VariantDef {
                model: entity_model::bake_sheep_model(),
                tex_variants: SHEEP_ADULT_TEX,
                tex_size: 64,
            },
            baby: Some(VariantDef {
                model: entity_model::bake_baby_sheep_model(),
                tex_variants: SHEEP_BABY_TEX,
                tex_size: 64,
            }),
            adult_overlays: vec![
                VariantDef {
                    model: entity_model::bake_sheep_wool_undercoat_model(),
                    tex_variants: SHEEP_WOOL_UNDERCOAT_TEX,
                    tex_size: 64,
                },
                VariantDef {
                    model: entity_model::bake_sheep_wool_model(),
                    tex_variants: SHEEP_WOOL_TEX,
                    tex_size: 64,
                },
            ],
            baby_overlays: vec![VariantDef {
                model: entity_model::bake_baby_sheep_wool_model(),
                tex_variants: SHEEP_BABY_WOOL_TEX,
                tex_size: 64,
            }],
        },
        MobDef {
            kind: EntityKind::Player,
            anim: AnimationType::Humanoid,
            adult: VariantDef {
                model: entity_model::bake_player_model(),
                tex_variants: PLAYER_TEX,
                tex_size: 64,
            },
            baby: None,
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
    ]
}

impl EntityRenderer {
    #[allow(clippy::too_many_arguments)]
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

        let push_constant_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::Vertex,
            offset: 0,
            size: 96,
        };

        let layouts = [camera_layout, texture_layout];
        let push_range = push_constant_range;
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            push_constant_range_count: 1,
            push_constant_ranges: &push_range,
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create entity pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let defs = mob_definitions();
        let tex_count: u32 = defs
            .iter()
            .map(|d| {
                let mut n = d.adult.tex_variants.len() as u32;
                if let Some(b) = &d.baby {
                    n += b.tex_variants.len() as u32;
                }
                for o in &d.adult_overlays {
                    n += o.tex_variants.len() as u32;
                }
                for o in &d.baby_overlays {
                    n += o.tex_variants.len() as u32;
                }
                n
            })
            .sum();

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: tex_count,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: MAX_FRAMES_IN_FLIGHT as u32 + tex_count,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create entity descriptor pool");

        let camera_layouts_vec: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| camera_layout).collect();
        let camera_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: camera_layouts_vec.len() as u32,
            set_layouts: camera_layouts_vec.as_ptr(),
            ..Default::default()
        };
        let mut camera_sets = vec![vk::DescriptorSet::null(); camera_layouts_vec.len()];
        device
            .allocate_descriptor_sets(&camera_alloc_info, &mut camera_sets)
            .expect("failed to allocate entity camera descriptor sets");

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                size_of::<CameraUniform>() as u64,
                "entity_camera_uniform",
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

        let texture_sampler = unsafe { util::create_nearest_sampler(device) };

        let mut mobs = HashMap::new();

        for def in defs {
            let mut build = |v: VariantDef| {
                build_variants(
                    device,
                    queue,
                    command_pool,
                    allocator,
                    descriptor_pool,
                    texture_layout,
                    texture_sampler,
                    jar_assets_dir,
                    asset_index,
                    v,
                )
            };
            let adult_variants = build(def.adult);
            let baby_variants = def.baby.map(&mut build);
            let adult_overlays: Vec<MobVariant> = def
                .adult_overlays
                .into_iter()
                .flat_map(&mut build)
                .collect();
            let baby_overlays: Vec<MobVariant> =
                def.baby_overlays.into_iter().flat_map(&mut build).collect();

            // Anim part-name indices are computed against the base variant's model and
            // reused for each overlay draw. Catch mismatched part order at construction
            // time rather than rendering wrong limbs in production.
            assert_part_order_matches(&adult_variants, &adult_overlays);
            if let Some(baby) = &baby_variants {
                assert_part_order_matches(baby, &baby_overlays);
            }

            mobs.insert(
                def.kind,
                MobEntry {
                    adult_variants,
                    baby_variants,
                    adult_overlays,
                    baby_overlays,
                    anim: def.anim,
                },
            );
        }

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            texture_layout,
            descriptor_pool,
            camera_sets,
            camera_buffers,
            camera_allocations,
            texture_sampler,
            mobs,
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);
    }

    pub fn draw(&self, cmd: vk::CommandBuffer, frame: usize, entities: &[EntityRenderInfo]) {
        if entities.is_empty() {
            return;
        }

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);

        let mut last_variant: *const MobVariant = std::ptr::null();
        for info in entities {
            let Some(entry) = self.mobs.get(&info.entity_kind) else {
                continue;
            };
            let variant = entry.base_variant(info.is_baby, info.variant_index);

            let entity_mat = glam::Mat4::from_translation(info.position.as_vec3())
                * glam::Mat4::from_rotation_y((180.0 - info.body_y_rot_deg).to_radians());

            let anim = match entry.anim {
                AnimationType::Quadruped => entity_model::compute_quadruped_anim(
                    &variant.model,
                    info.head_x_rot_deg,
                    info.head_y_rot_deg - info.body_y_rot_deg,
                    info.walk_anim_pos,
                    info.walk_anim_speed,
                    info.head_y_offset,
                    info.head_x_rot_deg_override,
                ),
                AnimationType::Humanoid => entity_model::compute_humanoid_anim(
                    &variant.model,
                    info.head_x_rot_deg,
                    info.head_y_rot_deg - info.body_y_rot_deg,
                    info.walk_anim_pos,
                    info.walk_anim_speed,
                    info.is_crouching,
                ),
            };

            let overlay_color = if info.has_red_overlay {
                HURT_OVERLAY
            } else {
                NO_OVERLAY
            };

            self.draw_variant(
                cmd,
                frame,
                variant,
                entity_mat,
                &anim,
                WHITE_TINT,
                overlay_color,
                &mut last_variant,
            );

            for (slot, overlay) in entry.overlays(info.is_baby).iter().enumerate() {
                let Some(tint) = info.overlay_tints[slot] else {
                    continue;
                };
                self.draw_variant(
                    cmd,
                    frame,
                    overlay,
                    entity_mat,
                    &anim,
                    tint,
                    overlay_color,
                    &mut last_variant,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_variant(
        &self,
        cmd: vk::CommandBuffer,
        frame: usize,
        variant: &MobVariant,
        entity_mat: glam::Mat4,
        anim: &entity_model::PartAnim,
        tint: [f32; 4],
        overlay_color: [f32; 4],
        last_variant: &mut *const MobVariant,
    ) {
        let ptr: *const MobVariant = variant;
        if *last_variant != ptr {
            cmd.bind_descriptor_sets(
                vk::PipelineBindPoint::Graphics,
                self.pipeline_layout,
                0,
                &[self.camera_sets[frame], variant.texture_set],
                &[],
            );
            cmd.bind_vertex_buffers(0, &[variant.vertex_buffer], &[0]);
            *last_variant = ptr;
        }

        let part_transforms = variant.model.compute_part_transforms(anim);
        for (i, (start, count)) in variant.model.part_ranges.iter().enumerate() {
            if *count == 0 {
                continue;
            }
            let part_mat = entity_mat * part_transforms[i];
            let mat_array = part_mat.to_cols_array();
            let mut bytes = [0u8; 96];
            bytes[..64].copy_from_slice(bytemuck::cast_slice(&mat_array));
            bytes[64..80].copy_from_slice(bytemuck::cast_slice(&tint));
            bytes[80..].copy_from_slice(bytemuck::cast_slice(&overlay_color));
            cmd.push_constants(
                self.pipeline_layout,
                vk::ShaderStageFlags::Vertex,
                0,
                &bytes,
            );
            cmd.draw(*count, 1, *start, 0);
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
            alloc
                .free(std::mem::replace(&mut self.camera_allocations[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }

        device.destroy_sampler(self.texture_sampler, None);

        for entry in self.mobs.values_mut() {
            let variants: Vec<&mut MobVariant> = entry
                .adult_variants
                .iter_mut()
                .chain(entry.baby_variants.iter_mut().flatten())
                .chain(entry.adult_overlays.iter_mut())
                .chain(entry.baby_overlays.iter_mut())
                .collect();
            for v in variants {
                device.destroy_buffer(v.vertex_buffer, None);
                alloc
                    .free(std::mem::replace(&mut v.vertex_allocation, unsafe {
                        std::mem::zeroed()
                    }))
                    .ok();
                device.destroy_image_view(v.texture_view, None);
                alloc
                    .free(std::mem::replace(&mut v.texture_allocation, unsafe {
                        std::mem::zeroed()
                    }))
                    .ok();
                device.destroy_image(v.texture_image, None);
            }
        }

        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.camera_layout, None);
        device.destroy_descriptor_set_layout(self.texture_layout, None);
    }
}

fn assert_part_order_matches(base: &[MobVariant], overlays: &[MobVariant]) {
    let Some(base_first) = base.first() else {
        return;
    };
    let base_names: Vec<&str> = base_first
        .model
        .parts
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    for overlay in overlays {
        let overlay_names: Vec<&str> = overlay
            .model
            .parts
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            base_names, overlay_names,
            "overlay part order must match base; anim indices are shared across both"
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn build_variants(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    descriptor_pool: vk::DescriptorPool,
    texture_layout: vk::DescriptorSetLayout,
    texture_sampler: vk::Sampler,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    variant: VariantDef,
) -> Vec<MobVariant> {
    let VariantDef {
        model,
        tex_variants,
        tex_size,
    } = variant;
    let vert_bytes = bytemuck::cast_slice::<ChunkVertex, u8>(&model.vertices);

    tex_variants
        .iter()
        .map(|tex_keys| {
            let (vertex_buffer, vertex_allocation) = util::create_mapped_buffer(
                device,
                allocator,
                vert_bytes,
                vk::BufferUsageFlags::VertexBuffer,
                "entity_vertices",
            );

            let (texture_image, texture_view, texture_allocation) = load_entity_texture(
                device,
                queue,
                command_pool,
                allocator,
                jar_assets_dir,
                asset_index,
                tex_keys,
                tex_size,
            );

            let tex_alloc_info = vk::DescriptorSetAllocateInfo {
                descriptor_pool,
                descriptor_set_count: 1,
                set_layouts: &texture_layout,
                ..Default::default()
            };
            let mut texture_set = vk::DescriptorSet::null();
            device
                .allocate_descriptor_sets(&tex_alloc_info, slice::from_mut(&mut texture_set))
                .expect("failed to allocate entity texture descriptor set");

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
                image_info: &image_info,
                ..Default::default()
            };
            device.update_descriptor_sets(&[tex_write], &[]);

            MobVariant {
                model: model.clone(),
                vertex_buffer,
                vertex_allocation,
                texture_image,
                texture_view,
                texture_allocation,
                texture_set,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn load_entity_texture(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    asset_keys: &[&str],
    fallback_size: u32,
) -> (vk::Image, vk::ImageView, Allocation) {
    let (pixels, width, height) = asset_keys
        .iter()
        .find_map(|key| {
            let path = resolve_asset_path(jar_assets_dir, asset_index, key);
            util::load_png(&path)
        })
        .unwrap_or_else(|| {
            tracing::warn!(
                "Failed to load entity texture {:?}, using fallback",
                asset_keys
            );
            fallback_texture(fallback_size)
        });

    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, width, height, "entity_texture");
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, &pixels, "entity_texture_staging");
    util::upload_image(
        device,
        queue,
        command_pool,
        staging_buf,
        image,
        width,
        height,
    );
    device.destroy_buffer(staging_buf, None);
    allocator.lock().unwrap().free(staging_alloc).ok();
    (image, view, allocation)
}

pub(super) fn fallback_texture(size: u32) -> (Vec<u8>, u32, u32) {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[219, 148, 148, 255]);
    }
    (pixels, size, size)
}

pub(super) fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("entity.vert.spv");
    let frag_spv = shader::include_spirv!("entity.frag.spv");

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

    let binding_descs = ChunkVertex::binding_description();
    let attr_descs = ChunkVertex::attribute_descriptions();

    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: 1,
        vertex_binding_descriptions: &binding_descs,
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
        depth_write_enable: vk::TRUE,
        depth_compare_op: vk::CompareOp::LessOrEqual,
        ..Default::default()
    };

    let blend_attachment = vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::FALSE,
        color_write_mask: vk::ColorComponentFlags::RGBA,
        ..Default::default()
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

    let mut pipeline = vk::Pipeline::null();
    device
        .create_graphics_pipelines(
            vk::PipelineCache::null(),
            &pipeline_info,
            None,
            slice::from_mut(&mut pipeline),
        )
        .expect("failed to create entity pipeline");

    device.destroy_shader_module(vert_module, None);
    device.destroy_shader_module(frag_module, None);

    pipeline
}
