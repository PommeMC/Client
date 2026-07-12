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

pub const MAX_OVERLAYS: usize = 4;

/// Per-frame instance buffer capacity, in (entity, part) draws. Far above any
/// realistic on-screen entity count; excess is dropped with a warning.
const MAX_INSTANCES: usize = 16384;
const MAX_PLAYER_SKINS: usize = 128;

/// Per-instance data for one (entity, part) draw, fed as instance-rate vertex
/// attributes (binding 1) — the four model-matrix columns, tint, overlay, uv.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct EntityInstance {
    model: [[f32; 4]; 4],
    tint: [f32; 4],
    overlay_color: [f32; 4],
    uv_params: [f32; 4],
}

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
    pub player_uuid: Option<uuid::Uuid>,
    pub variant_index: u32,
    pub overlay_tints: [Option<[f32; 4]>; MAX_OVERLAYS],
    /// Per-slot overlay texture variant (villager type/profession/level).
    pub overlay_variants: [u32; MAX_OVERLAYS],
    /// Villager head-shake (unhappy counter > 0).
    pub is_unhappy: bool,
    pub head_y_offset: f32,
    pub head_x_rot_deg_override: Option<f32>,
    pub has_red_overlay: bool,
    /// Mob is targeting/attacking — raises zombie/skeleton arms.
    pub aggressive: bool,
    /// Interpolated entity age in ticks; drives the undead idle arm bob.
    pub age_in_ticks: f32,
    /// Arm-swing progress 0..1; drives the zombie attack swing.
    pub attack_time: f32,
    /// Skip frustum/distance culling (the 3rd-person self entity, which sits at
    /// the camera and must never blink out).
    pub skip_cull: bool,
}

/// How an overlay layer is blended. Base/baby variants are always `Opaque`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OverlayKind {
    /// Cutout, depth-writing — sheep wool and all base models.
    Opaque,
    /// Translucent, full-bright, depth-write off — spider glowing eyes.
    EyesTranslucent,
    /// Additive, full-bright, depth-write off, scrolling UV — charged creeper
    /// swirl.
    SwirlAdditive,
}

struct MobVariant {
    model: BakedEntityModel,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Allocation,
    texture_image: vk::Image,
    texture_view: vk::ImageView,
    texture_allocation: Allocation,
    texture_set: vk::DescriptorSet,
    overlay_kind: OverlayKind,
}

struct MobEntry {
    adult_variants: Vec<MobVariant>,
    baby_variants: Option<Vec<MobVariant>>,
    /// Overlay slots, each with its own texture variants
    /// (`overlay_variants[slot]` picks one).
    adult_overlays: Vec<Vec<MobVariant>>,
    baby_overlays: Vec<Vec<MobVariant>>,
    anim: AnimationType,
}

struct PlayerSkinTexture {
    image: vk::Image,
    view: vk::ImageView,
    allocation: Allocation,
    set: vk::DescriptorSet,
    slim: bool,
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

    fn overlays(&self, is_baby: bool) -> &[Vec<MobVariant>] {
        if is_baby {
            &self.baby_overlays
        } else {
            &self.adult_overlays
        }
    }

    fn overlay_variant(&self, is_baby: bool, slot: usize, variant_index: u32) -> &MobVariant {
        let pool = &self.overlays(is_baby)[slot];
        let idx = (variant_index as usize).min(pool.len().saturating_sub(1));
        &pool[idx]
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
    /// Translucent, depth-write off — spider eyes.
    eyes_pipeline: vk::Pipeline,
    /// Additive, depth-write off — charged-creeper energy swirl.
    swirl_pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    texture_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Allocation>,
    /// Per-instance vertex buffer (bound at binding 1), one per frame in
    /// flight.
    instance_buffers: Vec<vk::Buffer>,
    instance_allocations: Vec<Allocation>,
    texture_sampler: vk::Sampler,
    /// REPEAT-wrap sampler for the scrolling swirl overlay.
    texture_sampler_repeat: vk::Sampler,
    mobs: HashMap<EntityKind, MobEntry>,
    player_skins: HashMap<uuid::Uuid, PlayerSkinTexture>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BlendMode {
    Opaque,
    Translucent,
    Additive,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimationType {
    Quadruped,
    Humanoid,
    Zombie,
    Skeleton,
    Spider,
    Villager,
}

struct VariantDef {
    model: BakedEntityModel,
    /// Outer slice: one entry per texture variant (variant_index). Inner slice:
    /// fallback chain of asset keys.
    tex_variants: &'static [&'static [&'static str]],
    tex_size: u32,
    overlay_kind: OverlayKind,
}

struct MobDef {
    kind: EntityKind,
    anim: AnimationType,
    adult: Vec<VariantDef>,
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
    const ZOMBIE_TEX: &[&[&str]] = &[&["minecraft/textures/entity/zombie/zombie.png"]];
    const SKELETON_TEX: &[&[&str]] = &[&["minecraft/textures/entity/skeleton/skeleton.png"]];
    const CREEPER_TEX: &[&[&str]] = &[&["minecraft/textures/entity/creeper/creeper.png"]];
    const CREEPER_ARMOR_TEX: &[&[&str]] =
        &[&["minecraft/textures/entity/creeper/creeper_armor.png"]];
    const SPIDER_TEX: &[&[&str]] = &[&["minecraft/textures/entity/spider/spider.png"]];
    const SPIDER_EYES_TEX: &[&[&str]] = &[&["minecraft/textures/entity/spider/spider_eyes.png"]];
    const VILLAGER_TEX: &[&[&str]] = &[&["minecraft/textures/entity/villager/villager.png"]];
    const VILLAGER_BABY_TEX: &[&[&str]] =
        &[&["minecraft/textures/entity/villager/villager_baby.png"]];
    // Indexed by the builtin VillagerKind registry order.
    const VILLAGER_TYPE_TEX: &[&[&str]] = &[
        &["minecraft/textures/entity/villager/type/desert.png"],
        &["minecraft/textures/entity/villager/type/jungle.png"],
        &["minecraft/textures/entity/villager/type/plains.png"],
        &["minecraft/textures/entity/villager/type/savanna.png"],
        &["minecraft/textures/entity/villager/type/snow.png"],
        &["minecraft/textures/entity/villager/type/swamp.png"],
        &["minecraft/textures/entity/villager/type/taiga.png"],
    ];
    const VILLAGER_BABY_TYPE_TEX: &[&[&str]] = &[
        &["minecraft/textures/entity/villager/baby/desert.png"],
        &["minecraft/textures/entity/villager/baby/jungle.png"],
        &["minecraft/textures/entity/villager/baby/plains.png"],
        &["minecraft/textures/entity/villager/baby/savanna.png"],
        &["minecraft/textures/entity/villager/baby/snow.png"],
        &["minecraft/textures/entity/villager/baby/swamp.png"],
        &["minecraft/textures/entity/villager/baby/taiga.png"],
    ];
    // Indexed by VillagerProfession registry order minus one ("none" has no
    // texture).
    const VILLAGER_PROFESSION_TEX: &[&[&str]] = &[
        &["minecraft/textures/entity/villager/profession/armorer.png"],
        &["minecraft/textures/entity/villager/profession/butcher.png"],
        &["minecraft/textures/entity/villager/profession/cartographer.png"],
        &["minecraft/textures/entity/villager/profession/cleric.png"],
        &["minecraft/textures/entity/villager/profession/farmer.png"],
        &["minecraft/textures/entity/villager/profession/fisherman.png"],
        &["minecraft/textures/entity/villager/profession/fletcher.png"],
        &["minecraft/textures/entity/villager/profession/leatherworker.png"],
        &["minecraft/textures/entity/villager/profession/librarian.png"],
        &["minecraft/textures/entity/villager/profession/mason.png"],
        &["minecraft/textures/entity/villager/profession/nitwit.png"],
        &["minecraft/textures/entity/villager/profession/shepherd.png"],
        &["minecraft/textures/entity/villager/profession/toolsmith.png"],
        &["minecraft/textures/entity/villager/profession/weaponsmith.png"],
    ];
    // Indexed by profession level 1-5 minus one.
    const VILLAGER_LEVEL_TEX: &[&[&str]] = &[
        &["minecraft/textures/entity/villager/profession_level/stone.png"],
        &["minecraft/textures/entity/villager/profession_level/iron.png"],
        &["minecraft/textures/entity/villager/profession_level/gold.png"],
        &["minecraft/textures/entity/villager/profession_level/emerald.png"],
        &["minecraft/textures/entity/villager/profession_level/diamond.png"],
    ];

    // Base and baby models, plus opaque overlays (sheep wool), are all Opaque.
    fn opaque(
        model: BakedEntityModel,
        tex_variants: &'static [&'static [&'static str]],
        tex_size: u32,
    ) -> VariantDef {
        VariantDef {
            model,
            tex_variants,
            tex_size,
            overlay_kind: OverlayKind::Opaque,
        }
    }

    vec![
        MobDef {
            kind: EntityKind::Pig,
            anim: AnimationType::Quadruped,
            adult: vec![opaque(entity_model::bake_pig_model(), PIG_ADULT_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_pig_model(),
                PIG_BABY_TEX,
                32,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Cow,
            anim: AnimationType::Quadruped,
            adult: vec![opaque(entity_model::bake_cow_model(), COW_ADULT_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_cow_model(),
                COW_BABY_TEX,
                64,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Sheep,
            anim: AnimationType::Quadruped,
            adult: vec![opaque(
                entity_model::bake_sheep_model(),
                SHEEP_ADULT_TEX,
                64,
            )],
            baby: Some(opaque(
                entity_model::bake_baby_sheep_model(),
                SHEEP_BABY_TEX,
                64,
            )),
            adult_overlays: vec![
                opaque(
                    entity_model::bake_sheep_wool_undercoat_model(),
                    SHEEP_WOOL_UNDERCOAT_TEX,
                    64,
                ),
                opaque(entity_model::bake_sheep_wool_model(), SHEEP_WOOL_TEX, 64),
            ],
            baby_overlays: vec![opaque(
                entity_model::bake_baby_sheep_wool_model(),
                SHEEP_BABY_WOOL_TEX,
                64,
            )],
        },
        MobDef {
            kind: EntityKind::Player,
            anim: AnimationType::Humanoid,
            // Variant 0 = classic (wide) arms, 1 = slim; picked per player from
            // the skin's model metadata (effective_variant_index).
            adult: vec![
                opaque(entity_model::bake_player_model(false), PLAYER_TEX, 64),
                opaque(entity_model::bake_player_model(true), PLAYER_TEX, 64),
            ],
            baby: None,
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Zombie,
            anim: AnimationType::Zombie,
            adult: vec![opaque(entity_model::bake_zombie_model(), ZOMBIE_TEX, 64)],
            baby: Some(opaque(
                entity_model::bake_baby_zombie_model(),
                ZOMBIE_TEX,
                64,
            )),
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Skeleton,
            anim: AnimationType::Skeleton,
            adult: vec![opaque(
                entity_model::bake_skeleton_model(),
                SKELETON_TEX,
                64,
            )],
            baby: None,
            adult_overlays: vec![],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Creeper,
            anim: AnimationType::Quadruped,
            adult: vec![opaque(entity_model::bake_creeper_model(), CREEPER_TEX, 64)],
            baby: None,
            // Slot 0: charged-creeper energy swirl (additive, scrolling), shown only
            // when `powered` (gated via overlay_tints in entity_extras).
            adult_overlays: vec![VariantDef {
                model: entity_model::bake_creeper_model(),
                tex_variants: CREEPER_ARMOR_TEX,
                tex_size: 64,
                overlay_kind: OverlayKind::SwirlAdditive,
            }],
            baby_overlays: vec![],
        },
        MobDef {
            kind: EntityKind::Villager,
            anim: AnimationType::Villager,
            adult: vec![opaque(
                entity_model::bake_villager_model(false),
                VILLAGER_TEX,
                64,
            )],
            baby: Some(opaque(
                entity_model::bake_baby_villager_model(false),
                VILLAGER_BABY_TEX,
                64,
            )),
            // Cutout layers over the base skin (vanilla `VillagerProfessionLayer`):
            // slot 0 = biome type, slot 1 = biome type on the no-hat model (used
            // when the profession texture brings its own hat), slot 2 =
            // profession, slot 3 = profession level badge. entity_extras gates
            // slot 0 xor 1 and picks each slot's texture variant.
            // TODO: CustomHeadLayer (worn head items) and CrossedArmsItemLayer
            // (held item) need a held-item layer first.
            adult_overlays: vec![
                opaque(
                    entity_model::bake_villager_model(false),
                    VILLAGER_TYPE_TEX,
                    64,
                ),
                opaque(
                    entity_model::bake_villager_model(true),
                    VILLAGER_TYPE_TEX,
                    64,
                ),
                opaque(
                    entity_model::bake_villager_model(false),
                    VILLAGER_PROFESSION_TEX,
                    64,
                ),
                opaque(
                    entity_model::bake_villager_model(false),
                    VILLAGER_LEVEL_TEX,
                    64,
                ),
            ],
            baby_overlays: vec![
                opaque(
                    entity_model::bake_baby_villager_model(false),
                    VILLAGER_BABY_TYPE_TEX,
                    64,
                ),
                opaque(
                    entity_model::bake_baby_villager_model(true),
                    VILLAGER_BABY_TYPE_TEX,
                    64,
                ),
            ],
        },
        MobDef {
            kind: EntityKind::Spider,
            anim: AnimationType::Spider,
            adult: vec![opaque(entity_model::bake_spider_model(), SPIDER_TEX, 64)],
            baby: None,
            // Slot 0: glowing eyes (translucent, full-bright), always visible.
            adult_overlays: vec![VariantDef {
                model: entity_model::bake_spider_model(),
                tex_variants: SPIDER_EYES_TEX,
                tex_size: 64,
                overlay_kind: OverlayKind::EyesTranslucent,
            }],
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
        let layouts = [camera_layout, texture_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create entity pipeline layout");

        let [pipeline, eyes_pipeline, swirl_pipeline] =
            create_pipelines(device, render_pass, pipeline_layout);

        let defs = mob_definitions();
        let tex_count: u32 = defs
            .iter()
            .map(|d| {
                let mut n: u32 = d.adult.iter().map(|v| v.tex_variants.len() as u32).sum();
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
        let tex_count = tex_count + MAX_PLAYER_SKINS as u32;

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
            flags: vk::DescriptorPoolCreateFlags::FreeDescriptorSet,
            max_sets: MAX_FRAMES_IN_FLIGHT as u32 + tex_count,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create entity descriptor pool");

        let (camera_sets, camera_buffers, camera_allocations) =
            create_camera_sets(device, allocator, descriptor_pool, camera_layout);
        // Per-instance data is a vertex buffer (bound at binding 1), not an SSBO:
        // MoltenVK can't translate a storage-buffer read in a vertex shader.
        let (instance_buffers, instance_allocations) = create_per_frame_host_buffers(
            device,
            allocator,
            (MAX_INSTANCES * size_of::<EntityInstance>()) as u64,
            vk::BufferUsageFlags::VertexBuffer,
            "entity_instances",
        );

        let texture_sampler = unsafe { util::create_nearest_sampler(device) };
        let texture_sampler_repeat = unsafe { util::create_nearest_repeat_sampler(device) };

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
                    texture_sampler_repeat,
                    jar_assets_dir,
                    asset_index,
                    v,
                )
            };
            let adult_variants: Vec<MobVariant> =
                def.adult.into_iter().flat_map(&mut build).collect();
            let baby_variants = def.baby.map(&mut build);
            let adult_overlays: Vec<Vec<MobVariant>> =
                def.adult_overlays.into_iter().map(&mut build).collect();
            let baby_overlays: Vec<Vec<MobVariant>> =
                def.baby_overlays.into_iter().map(&mut build).collect();

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
            eyes_pipeline,
            swirl_pipeline,
            pipeline_layout,
            camera_layout,
            texture_layout,
            descriptor_pool,
            camera_sets,
            camera_buffers,
            camera_allocations,
            instance_buffers,
            instance_allocations,
            texture_sampler,
            texture_sampler_repeat,
            mobs,
            player_skins: HashMap::new(),
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);
    }

    pub fn update_player_skin(
        &mut self,
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        allocator: &Arc<Mutex<Allocator>>,
        uuid: &uuid::Uuid,
        skin: &crate::renderer::SkinData,
    ) {
        if !self.player_skins.contains_key(uuid) && self.player_skins.len() >= MAX_PLAYER_SKINS {
            tracing::warn!("Player skin cache full; keeping fallback texture for {uuid}");
            return;
        }

        let (image, view, allocation) = upload_texture_pixels(
            device,
            queue,
            command_pool,
            allocator,
            &skin.pixels,
            skin.width,
            skin.height,
        );
        let set = if let Some(old) = self.player_skins.get(uuid) {
            old.set
        } else {
            let tex_alloc_info = vk::DescriptorSetAllocateInfo {
                descriptor_pool: self.descriptor_pool,
                descriptor_set_count: 1,
                set_layouts: &self.texture_layout,
                ..Default::default()
            };
            let mut texture_set = vk::DescriptorSet::null();
            device
                .allocate_descriptor_sets(&tex_alloc_info, slice::from_mut(&mut texture_set))
                .expect("failed to allocate player skin texture descriptor set");
            texture_set
        };

        let image_info = vk::DescriptorImageInfo {
            sampler: self.texture_sampler,
            image_view: view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let tex_write = vk::WriteDescriptorSet {
            dst_set: set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            image_info: &image_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[tex_write], &[]);

        if let Some(old) = self.player_skins.insert(
            uuid.to_owned(),
            PlayerSkinTexture {
                image,
                view,
                allocation,
                set,
                slim: skin.slim,
            },
        ) {
            device.destroy_image_view(old.view, None);
            device.destroy_image(old.image, None);
            allocator.lock().unwrap().free(old.allocation).ok();
        }

        tracing::debug!(
            "Player skin loaded for {uuid}: {}x{}",
            skin.width,
            skin.height
        );
    }

    pub fn remove_player_skin(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        uuid: &uuid::Uuid,
    ) {
        if let Some(skin) = self.player_skins.remove(uuid) {
            free_player_skin_texture(device, allocator, self.descriptor_pool, skin);
        }
    }

    pub fn clear_player_skins(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let descriptor_pool = self.descriptor_pool;
        for (_, skin) in self.player_skins.drain() {
            free_player_skin_texture(device, allocator, descriptor_pool, skin);
        }
    }

    fn player_skin(&self, info: &EntityRenderInfo) -> Option<&PlayerSkinTexture> {
        if info.entity_kind != EntityKind::Player {
            return None;
        }
        self.player_skins.get(info.player_uuid.as_ref()?)
    }

    fn player_texture_set(
        &self,
        info: &EntityRenderInfo,
        fallback: vk::DescriptorSet,
    ) -> vk::DescriptorSet {
        self.player_skin(info).map_or(fallback, |skin| skin.set)
    }

    /// Players pick their model variant (0 = wide, 1 = slim) from the fetched
    /// skin's metadata rather than the caller-supplied index.
    fn effective_variant_index(&self, info: &EntityRenderInfo) -> u32 {
        self.player_skin(info)
            .map_or(info.variant_index, |skin| skin.slim as u32)
    }

    fn compute_anim(
        &self,
        anim_type: AnimationType,
        model: &BakedEntityModel,
        info: &EntityRenderInfo,
    ) -> entity_model::PartAnim {
        let local_head_y = info.head_y_rot_deg - info.body_y_rot_deg;
        match anim_type {
            AnimationType::Quadruped => entity_model::compute_quadruped_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.head_y_offset,
                info.head_x_rot_deg_override,
            ),
            AnimationType::Humanoid => entity_model::compute_humanoid_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.is_crouching,
            ),
            AnimationType::Zombie => entity_model::compute_zombie_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.aggressive,
                info.age_in_ticks,
                info.attack_time,
            ),
            AnimationType::Skeleton => entity_model::compute_skeleton_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.aggressive,
                info.age_in_ticks,
            ),
            AnimationType::Spider => entity_model::compute_spider_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
            ),
            AnimationType::Villager => entity_model::compute_villager_anim(
                model,
                info.head_x_rot_deg,
                local_head_y,
                info.walk_anim_pos,
                info.walk_anim_speed,
                info.is_unhappy,
                info.age_in_ticks,
            ),
        }
    }

    /// The translation is anchor-relative, subtracted in f64 (see
    /// `Camera::anchor`).
    fn entity_matrix(info: &EntityRenderInfo, anchor: glam::DVec3) -> glam::Mat4 {
        glam::Mat4::from_translation((*info.position - anchor).as_vec3())
            * glam::Mat4::from_rotation_y((180.0 - info.body_y_rot_deg).to_radians())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        entities: &[EntityRenderInfo],
        frustum: &[[f32; 4]; 6],
        anchor: glam::DVec3,
        eye: glam::DVec3,
        cull_dist: f32,
    ) {
        if entities.is_empty() {
            return;
        }

        // Build the per-frame instance buffer + draw records on the CPU
        // (immutable reads of self.mobs), grouped by variant so each (variant,
        // part) becomes a single instanced draw. `vis`/`groups` borrow self.mobs
        // and are dropped at the end of this block, before the buffer write below.
        let cull_dist_sq = cull_dist * cull_dist;
        let mut instances: Vec<EntityInstance> = Vec::new();
        let (opaque, eyes, swirl) = {
            let mut vis: Vec<VisEntity> = Vec::new();
            for info in entities {
                let Some(entry) = self.mobs.get(&info.entity_kind) else {
                    continue;
                };
                if !info.skip_cull && !entity_visible(info, frustum, eye, cull_dist_sq) {
                    continue;
                }
                let variant = entry.base_variant(info.is_baby, self.effective_variant_index(info));
                let entity_mat = Self::entity_matrix(info, anchor);
                let anim = self.compute_anim(entry.anim, &variant.model, info);
                vis.push(VisEntity {
                    info,
                    entry,
                    entity_mat,
                    anim,
                });
            }
            if vis.is_empty() {
                return;
            }

            // Opaque pass: base model + opaque overlays (sheep wool, villager
            // clothing). Overlay layers are exactly coplanar with the base and
            // rely on LessOrEqual depth + draw order to win, so emit in layer
            // phases (all bases, then slot 0 across all entities, then slot
            // 1, ...) — interleaving per entity would let a shared group
            // created by an earlier entity draw a later entity's lower layer
            // after its upper one.
            let hurt_color = |info: &EntityRenderInfo| {
                if info.has_red_overlay {
                    HURT_OVERLAY
                } else {
                    NO_OVERLAY
                }
            };
            let mut opaque = VariantGroups::default();
            for (vi, v) in vis.iter().enumerate() {
                let base = v
                    .entry
                    .base_variant(v.info.is_baby, self.effective_variant_index(v.info));
                let texture_set = self.player_texture_set(v.info, base.texture_set);
                opaque.add(
                    base,
                    texture_set,
                    (vi, WHITE_TINT, hurt_color(v.info), [0.0, 0.0]),
                );
            }
            for slot in 0..MAX_OVERLAYS {
                for (vi, v) in vis.iter().enumerate() {
                    if slot >= v.entry.overlays(v.info.is_baby).len() {
                        continue;
                    }
                    let overlay = v.entry.overlay_variant(
                        v.info.is_baby,
                        slot,
                        v.info.overlay_variants[slot],
                    );
                    if overlay.overlay_kind != OverlayKind::Opaque {
                        continue;
                    }
                    if let Some(tint) = v.info.overlay_tints[slot] {
                        opaque.add(
                            overlay,
                            overlay.texture_set,
                            (vi, tint, hurt_color(v.info), [0.0, 0.0]),
                        );
                    }
                }
            }

            let eyes = collect_emissive(&vis, OverlayKind::EyesTranslucent);
            let swirl = collect_emissive(&vis, OverlayKind::SwirlAdditive);

            (
                opaque.emit(&vis, &mut instances),
                eyes.emit(&vis, &mut instances),
                swirl.emit(&vis, &mut instances),
            )
        };

        // Write the instance buffer (clamped to capacity; the cap is far above any
        // realistic entity count, so overflow only drops the tail with a warning).
        let count = instances.len().min(MAX_INSTANCES);
        if instances.len() > MAX_INSTANCES {
            tracing::warn!(
                "Entity instances ({}) exceed cap {}, dropping excess",
                instances.len(),
                MAX_INSTANCES
            );
        }
        let bytes = bytemuck::cast_slice(&instances[..count]);
        self.instance_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);

        self.record_pass(cmd, frame, self.pipeline, &opaque, count);
        self.record_pass(cmd, frame, self.eyes_pipeline, &eyes, count);
        self.record_pass(cmd, frame, self.swirl_pipeline, &swirl, count);
    }

    fn record_pass(
        &self,
        cmd: vk::CommandBuffer,
        frame: usize,
        pipeline: vk::Pipeline,
        records: &[DrawRecord],
        count: usize,
    ) {
        if records.is_empty() {
            return;
        }
        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, pipeline);
        // Per-instance data (binding 1) is the same buffer for the whole pass;
        // gl_InstanceIndex (incl. firstInstance) indexes into it.
        cmd.bind_vertex_buffers(1, &[self.instance_buffers[frame]], &[0]);
        let mut last_vb = vk::Buffer::null();
        let mut last_texture_set = vk::DescriptorSet::null();
        for r in records {
            if r.first_instance as usize + r.instance_count as usize > count {
                continue; // dropped by the capacity clamp above
            }
            if r.vertex_buffer != last_vb || r.texture_set != last_texture_set {
                cmd.bind_descriptor_sets(
                    vk::PipelineBindPoint::Graphics,
                    self.pipeline_layout,
                    0,
                    &[self.camera_sets[frame], r.texture_set],
                    &[],
                );
                cmd.bind_vertex_buffers(0, &[r.vertex_buffer], &[0]);
                last_vb = r.vertex_buffer;
                last_texture_set = r.texture_set;
            }
            cmd.draw(
                r.part_count,
                r.instance_count,
                r.part_start,
                r.first_instance,
            );
        }
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline(self.eyes_pipeline, None);
        device.destroy_pipeline(self.swirl_pipeline, None);
        [self.pipeline, self.eyes_pipeline, self.swirl_pipeline] =
            create_pipelines(device, render_pass, self.pipeline_layout);
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
            device.destroy_buffer(self.instance_buffers[i], None);
            alloc
                .free(std::mem::replace(
                    &mut self.instance_allocations[i],
                    unsafe { std::mem::zeroed() },
                ))
                .ok();
        }

        device.destroy_sampler(self.texture_sampler, None);
        device.destroy_sampler(self.texture_sampler_repeat, None);

        for entry in self.mobs.values_mut() {
            let variants: Vec<&mut MobVariant> = entry
                .adult_variants
                .iter_mut()
                .chain(entry.baby_variants.iter_mut().flatten())
                .chain(entry.adult_overlays.iter_mut().flatten())
                .chain(entry.baby_overlays.iter_mut().flatten())
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
        for (_, skin) in self.player_skins.drain() {
            destroy_player_skin_texture(device, &mut alloc, skin);
        }

        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline(self.eyes_pipeline, None);
        device.destroy_pipeline(self.swirl_pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.camera_layout, None);
        device.destroy_descriptor_set_layout(self.texture_layout, None);
    }
}

/// One host-visible buffer per frame in flight.
fn create_per_frame_host_buffers(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    size: u64,
    usage: vk::BufferUsageFlags,
    name: &str,
) -> (Vec<vk::Buffer>, Vec<Allocation>) {
    let mut buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
    let mut allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
    for _ in 0..MAX_FRAMES_IN_FLIGHT {
        let (buf, alloc) = util::create_host_buffer(device, allocator, size, usage, name);
        buffers.push(buf);
        allocations.push(alloc);
    }
    (buffers, allocations)
}

/// Per-frame camera UBOs, each bound to its own descriptor set at binding 0.
fn create_camera_sets(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
) -> (Vec<vk::DescriptorSet>, Vec<vk::Buffer>, Vec<Allocation>) {
    let layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| layout).collect();
    let alloc_info = vk::DescriptorSetAllocateInfo {
        descriptor_pool: pool,
        descriptor_set_count: layouts.len() as u32,
        set_layouts: layouts.as_ptr(),
        ..Default::default()
    };
    let mut sets = vec![vk::DescriptorSet::null(); layouts.len()];
    device
        .allocate_descriptor_sets(&alloc_info, &mut sets)
        .expect("failed to allocate entity camera descriptor sets");

    let size = size_of::<CameraUniform>() as u64;
    let (buffers, allocations) = create_per_frame_host_buffers(
        device,
        allocator,
        size,
        vk::BufferUsageFlags::UniformBuffer,
        "entity_camera_uniform",
    );
    for (&set, &buffer) in sets.iter().zip(&buffers) {
        let buffer_info = vk::DescriptorBufferInfo {
            buffer,
            offset: 0,
            range: size,
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
    }
    (sets, buffers, allocations)
}

/// A culled, drawable entity with its world transform and animation
/// precomputed.
struct VisEntity<'a> {
    info: &'a EntityRenderInfo,
    entry: &'a MobEntry,
    entity_mat: glam::Mat4,
    anim: entity_model::PartAnim,
}

/// One instanced (variant, part) draw: a run of `instance_count` instances from
/// `first_instance` in the per-frame instance buffer.
struct DrawRecord {
    texture_set: vk::DescriptorSet,
    vertex_buffer: vk::Buffer,
    part_start: u32,
    part_count: u32,
    first_instance: u32,
    instance_count: u32,
}

/// (visible-entity index, tint, overlay color, uv scroll) for one instance.
type Member = (usize, [f32; 4], [f32; 4], [f32; 2]);

/// Visible entities grouped by variant (geometry), so each variant's parts emit
/// one instanced draw covering all its entities.
#[derive(Default)]
struct VariantGroups<'a> {
    groups: Vec<(&'a MobVariant, vk::DescriptorSet, Vec<Member>)>,
}

impl<'a> VariantGroups<'a> {
    fn add(&mut self, variant: &'a MobVariant, texture_set: vk::DescriptorSet, member: Member) {
        let key = variant as *const MobVariant as usize;
        let gi =
            match self.groups.iter().position(|(v, set, _)| {
                *v as *const MobVariant as usize == key && *set == texture_set
            }) {
                Some(gi) => gi,
                None => {
                    self.groups.push((variant, texture_set, Vec::new()));
                    self.groups.len() - 1
                }
            };
        self.groups[gi].2.push(member);
    }

    fn emit(&self, vis: &[VisEntity], instances: &mut Vec<EntityInstance>) -> Vec<DrawRecord> {
        let mut records = Vec::new();
        for (variant, texture_set, members) in &self.groups {
            // Part transforms differ per entity (animation), so compute per member.
            let pts: Vec<Vec<glam::Mat4>> = members
                .iter()
                .map(|(vi, ..)| variant.model.compute_part_transforms(&vis[*vi].anim))
                .collect();
            for (p, (start, part_count)) in variant.model.part_ranges.iter().enumerate() {
                if *part_count == 0 {
                    continue;
                }
                let first_instance = instances.len() as u32;
                for (k, (vi, tint, overlay, uv)) in members.iter().enumerate() {
                    let model = vis[*vi].entity_mat * pts[k][p];
                    instances.push(EntityInstance {
                        model: model.to_cols_array_2d(),
                        tint: *tint,
                        overlay_color: *overlay,
                        uv_params: [uv[0], uv[1], 0.0, 0.0],
                    });
                }
                records.push(DrawRecord {
                    texture_set: *texture_set,
                    vertex_buffer: variant.vertex_buffer,
                    part_start: *start,
                    part_count: *part_count,
                    first_instance,
                    instance_count: members.len() as u32,
                });
            }
        }
        records
    }
}

/// Group the emissive overlays of one kind (eyes / swirl) by variant.
fn collect_emissive<'a>(vis: &[VisEntity<'a>], kind: OverlayKind) -> VariantGroups<'a> {
    let mut groups = VariantGroups::default();
    for (vi, v) in vis.iter().enumerate() {
        // Energy swirl scrolls its UVs over time (vanilla `EnergySwirlLayer`).
        let uv = if kind == OverlayKind::SwirlAdditive {
            let o = (v.info.age_in_ticks * 0.01).rem_euclid(1.0);
            [o, o]
        } else {
            [0.0, 0.0]
        };
        for slot in 0..v.entry.overlays(v.info.is_baby).len() {
            let overlay =
                v.entry
                    .overlay_variant(v.info.is_baby, slot, v.info.overlay_variants[slot]);
            if overlay.overlay_kind != kind {
                continue;
            }
            if let Some(tint) = v.info.overlay_tints[slot] {
                groups.add(overlay, overlay.texture_set, (vi, tint, NO_OVERLAY, uv));
            }
        }
    }
    groups
}

const ANIM_MARGIN: f32 = 0.5;

/// Vanilla (width, height) hitbox per supported mob, scaled for babies; used to
/// build the cull bounding sphere.
fn entity_bounds(kind: EntityKind, is_baby: bool) -> (f32, f32) {
    let (w, h) = match kind {
        EntityKind::Pig => (0.9, 0.9),
        EntityKind::Cow => (0.9, 1.4),
        EntityKind::Sheep => (0.9, 1.3),
        EntityKind::Zombie => (0.6, 1.95),
        EntityKind::Skeleton => (0.6, 1.99),
        EntityKind::Creeper => (0.6, 1.7),
        EntityKind::Spider => (1.4, 0.9),
        EntityKind::Villager => (0.6, 1.95),
        EntityKind::Player => (0.6, 1.8),
        _ => (1.0, 1.0),
    };
    let s = if is_baby { 0.5 } else { 1.0 };
    (w * s, h * s)
}

/// Bounding-sphere frustum + distance cull. The frustum planes operate on
/// camera-relative coords (like chunk cull), so the entity position is
/// rebased against the eye in f64 first.
fn entity_visible(
    info: &EntityRenderInfo,
    frustum: &[[f32; 4]; 6],
    eye: glam::DVec3,
    cull_dist_sq: f32,
) -> bool {
    let (w, h) = entity_bounds(info.entity_kind, info.is_baby);
    let radius = 0.5 * (2.0 * w * w + h * h).sqrt() + ANIM_MARGIN;
    let mut q = (*info.position - eye).as_vec3();
    q.y += h * 0.5;
    if q.length_squared() > cull_dist_sq {
        return false;
    }
    for pl in frustum {
        if pl[0] * q.x + pl[1] * q.y + pl[2] * q.z + pl[3] < -radius {
            return false;
        }
    }
    true
}

fn assert_part_order_matches(base: &[MobVariant], overlays: &[Vec<MobVariant>]) {
    let Some(base_first) = base.first() else {
        return;
    };
    let base_names: Vec<&str> = base_first
        .model
        .parts
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    for overlay in overlays.iter().flatten() {
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
    texture_sampler_repeat: vk::Sampler,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    variant: VariantDef,
) -> Vec<MobVariant> {
    let VariantDef {
        model,
        tex_variants,
        tex_size,
        overlay_kind,
    } = variant;
    // The scrolling swirl needs REPEAT wrapping; everything else clamps.
    let sampler = match overlay_kind {
        OverlayKind::SwirlAdditive => texture_sampler_repeat,
        _ => texture_sampler,
    };
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
                sampler,
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
                overlay_kind,
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

fn upload_texture_pixels(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> (vk::Image, vk::ImageView, Allocation) {
    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, width, height, "player_skin_texture");
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, pixels, "player_skin_texture_staging");
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

fn free_player_skin_texture(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    descriptor_pool: vk::DescriptorPool,
    skin: PlayerSkinTexture,
) {
    device
        .free_descriptor_sets(descriptor_pool, &[skin.set])
        .ok();
    let mut alloc = allocator.lock().unwrap();
    destroy_player_skin_texture(device, &mut alloc, skin);
}

fn destroy_player_skin_texture(
    device: &vk::Device,
    allocator: &mut Allocator,
    skin: PlayerSkinTexture,
) {
    device.destroy_image_view(skin.view, None);
    allocator.free(skin.allocation).ok();
    device.destroy_image(skin.image, None);
}

pub(super) fn fallback_texture(size: u32) -> (Vec<u8>, u32, u32) {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[219, 148, 148, 255]);
    }
    (pixels, size, size)
}

/// The entity render pipelines, in draw order: opaque base, translucent eyes,
/// additive swirl.
fn create_pipelines(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> [vk::Pipeline; 3] {
    [
        create_pipeline(
            device,
            render_pass,
            layout,
            BlendMode::Opaque,
            ModelInput::Instanced,
        ),
        create_pipeline(
            device,
            render_pass,
            layout,
            BlendMode::Translucent,
            ModelInput::Instanced,
        ),
        create_pipeline(
            device,
            render_pass,
            layout,
            BlendMode::Additive,
            ModelInput::Instanced,
        ),
    ]
}

/// Source of a draw's model matrix: mobs are GPU-instanced (binding 1, a perf
/// divergence from vanilla); block entities keep vanilla's per-draw
/// push-constant transform (binding 0 only).
pub(super) enum ModelInput {
    Instanced,
    PushConstant,
}

pub(super) fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
    blend: BlendMode,
    model_input: ModelInput,
) -> vk::Pipeline {
    let vert_spv: &[u8] = match model_input {
        ModelInput::Instanced => shader::include_spirv!("entity.vert.spv"),
        ModelInput::PushConstant => shader::include_spirv!("block_entity.vert.spv"),
    };
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

    // Binding 0: per-vertex mesh data. Instanced pipelines add binding 1 with
    // per-instance data (model columns + tint + overlay + uv), one EntityInstance
    // per (entity, part); push-constant ones bind only the mesh.
    let mut bindings = vec![ChunkVertex::binding_description()];
    let mut attrs = ChunkVertex::attribute_descriptions().to_vec();
    if let ModelInput::Instanced = model_input {
        bindings.push(vk::VertexInputBindingDescription {
            binding: 1,
            stride: size_of::<EntityInstance>() as u32,
            input_rate: vk::VertexInputRate::Instance,
        });
        for i in 0..7u32 {
            attrs.push(vk::VertexInputAttributeDescription {
                location: 3 + i,
                binding: 1,
                format: vk::Format::R32G32B32A32Sfloat,
                offset: i * 16,
            });
        }
    }

    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: bindings.len() as u32,
        vertex_binding_descriptions: bindings.as_ptr(),
        vertex_attribute_description_count: attrs.len() as u32,
        vertex_attribute_descriptions: attrs.as_ptr(),
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

    // Only the translucent eyes overlay skips depth-write (vanilla `EYES`); the
    // opaque base and additive swirl write depth (vanilla `ENERGY_SWIRL`).
    let depth_write = if blend == BlendMode::Translucent {
        vk::FALSE
    } else {
        vk::TRUE
    };
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo {
        depth_test_enable: vk::TRUE,
        depth_write_enable: depth_write,
        depth_compare_op: vk::CompareOp::LessOrEqual,
        ..Default::default()
    };

    let blend_attachment = match blend {
        BlendMode::Opaque => vk::PipelineColorBlendAttachmentState {
            blend_enable: vk::FALSE,
            color_write_mask: vk::ColorComponentFlags::RGBA,
            ..Default::default()
        },
        // Standard src-alpha over (glowing eyes).
        BlendMode::Translucent => vk::PipelineColorBlendAttachmentState {
            blend_enable: vk::TRUE,
            src_color_blend_factor: vk::BlendFactor::SrcAlpha,
            dst_color_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
            color_blend_op: vk::BlendOp::Add,
            src_alpha_blend_factor: vk::BlendFactor::One,
            dst_alpha_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
            alpha_blend_op: vk::BlendOp::Add,
            color_write_mask: vk::ColorComponentFlags::RGBA,
        },
        // Additive (energy swirl glow).
        BlendMode::Additive => vk::PipelineColorBlendAttachmentState {
            blend_enable: vk::TRUE,
            src_color_blend_factor: vk::BlendFactor::SrcAlpha,
            dst_color_blend_factor: vk::BlendFactor::One,
            color_blend_op: vk::BlendOp::Add,
            src_alpha_blend_factor: vk::BlendFactor::One,
            dst_alpha_blend_factor: vk::BlendFactor::One,
            alpha_blend_op: vk::BlendOp::Add,
            color_write_mask: vk::ColorComponentFlags::RGBA,
        },
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
